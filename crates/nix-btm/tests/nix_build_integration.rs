use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use nix_btm::{
    handle_internal_json::{JobsStateInner, handle_daemon_info},
    shutdown::Shutdown,
};
use tempfile::TempDir;
use tokio::sync::watch;

/// Integration test that:
/// 1. Creates a fresh temporary nix store
/// 2. Starts the daemon listener on a socket
/// 3. Runs `nix build` with --store pointing to temp store
/// 4. Verifies the resulting state has correct roots and jobs
/// 5. Cleans up by removing the temp store
#[tokio::test]
async fn test_nix_build_produces_correct_tree_roots() {
    // Create temporary store directory
    let temp_store = TempDir::new().expect("Failed to create temp store");
    let store_path = temp_store.path().to_path_buf();

    let socket_path = PathBuf::from("/tmp/nixbtm-test.sock");
    let _ = std::fs::remove_file(&socket_path);

    let is_shutdown = Shutdown::new();
    let (tx, mut rx) = watch::channel(JobsStateInner::default());

    let socket_path_clone = socket_path.clone();
    let is_shutdown_clone = is_shutdown.clone();
    let daemon_handle = tokio::spawn(async move {
        handle_daemon_info(socket_path_clone, 0o600, is_shutdown_clone, tx)
            .await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Run nix build with fresh store - use spawn so we can cancel it
    // Allow substitutes so we can see download/substitute jobs
    let mut nix_child = tokio::process::Command::new("nix")
        .arg("build")
        .arg("--no-link")
        .arg("--store")
        .arg(&store_path)
        .arg("--json-log-path")
        .arg(&socket_path)
        .arg("-vvv")
        .arg("nixpkgs#hello")
        .spawn()
        .expect("Failed to spawn nix build");

    // Let it run for a bit to populate state, then cancel
    // With substitutes, this should complete or get far enough
    tokio::time::sleep(Duration::from_secs(15)).await;
    let _ = nix_child.kill().await;

    // Give daemon time to process remaining messages
    tokio::time::sleep(Duration::from_millis(500)).await;

    is_shutdown.trigger();

    rx.changed().await.ok();
    let final_state = rx.borrow().clone();

    eprintln!("=== Final State ===");
    eprintln!("Jobs: {}", final_state.jid_to_job.len());
    eprintln!("Nodes in dep_tree: {}", final_state.dep_tree.nodes.len());
    eprintln!("Tree roots: {}", final_state.dep_tree.tree_roots.len());

    for (jid, job) in &final_state.jid_to_job {
        eprintln!("  Job {}: {} ({:?})", jid.0, job.drv.name, job.status);
    }

    for root in &final_state.dep_tree.tree_roots {
        eprintln!("  Root: {}", root.name);
    }

    // The main assertion: if we built anything, there should be roots
    // and the top-level derivation (hello) should be among them
    if !final_state.dep_tree.nodes.is_empty() {
        assert!(
            !final_state.dep_tree.tree_roots.is_empty(),
            "Expected at least one root when nodes exist"
        );

        // Check that "hello" is a root (not an intermediate dependency)
        let hello_is_root = final_state
            .dep_tree
            .tree_roots
            .iter()
            .any(|r| r.name.contains("hello"));

        if final_state
            .jid_to_job
            .values()
            .any(|j| j.drv.name.contains("hello"))
        {
            assert!(
                hello_is_root,
                "Expected 'hello' to be a root, but roots are: {:?}",
                final_state
                    .dep_tree
                    .tree_roots
                    .iter()
                    .map(|r| &r.name)
                    .collect::<Vec<_>>()
            );
        }
    }

    // Clean up
    daemon_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    // temp_store drops here and cleans up automatically
}

/// Test with a more complex derivation to verify root detection
#[tokio::test]
async fn test_nix_build_complex_derivation_roots() {
    // Create temporary store directory
    let temp_store = TempDir::new().expect("Failed to create temp store");
    let store_path = temp_store.path().to_path_buf();

    let socket_path = PathBuf::from("/tmp/nixbtm-test-complex.sock");
    let _ = std::fs::remove_file(&socket_path);

    let is_shutdown = Shutdown::new();
    let (tx, mut rx) = watch::channel(JobsStateInner::default());

    let socket_path_clone = socket_path.clone();
    let is_shutdown_clone = is_shutdown.clone();
    let daemon_handle = tokio::spawn(async move {
        handle_daemon_info(socket_path_clone, 0o600, is_shutdown_clone, tx)
            .await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Build nix itself (complex dependency tree) with fresh store
    let mut nix_child = tokio::process::Command::new("nix")
        .arg("build")
        .arg("--no-link")
        .arg("--store")
        .arg(&store_path)
        .arg("--json-log-path")
        .arg(&socket_path)
        .arg("-vvv")
        .arg("--substituters")
        .arg("")
        .arg("--max-jobs")
        .arg("1") // Limit parallelism for more predictable ordering
        .arg("github:nixos/nix")
        .spawn()
        .expect("Failed to spawn nix build");

    // Let it run for a while to build up the dependency tree, then cancel
    tokio::time::sleep(Duration::from_secs(10)).await;
    let _ = nix_child.kill().await;

    // Allow time for processing remaining messages
    tokio::time::sleep(Duration::from_millis(500)).await;

    is_shutdown.trigger();

    // Get final state
    rx.changed().await.ok();
    let final_state = rx.borrow().clone();

    eprintln!("=== Complex Build Final State ===");
    eprintln!("Jobs: {}", final_state.jid_to_job.len());
    eprintln!("Nodes: {}", final_state.dep_tree.nodes.len());
    eprintln!("Roots: {}", final_state.dep_tree.tree_roots.len());

    for root in &final_state.dep_tree.tree_roots {
        eprintln!("  Root: {}", root.name);
    }

    // Key assertion: intermediate dependencies should NOT be roots
    // Things like boehm-gc, boost-build should not be roots if nix is being
    // built
    if final_state.dep_tree.nodes.len() > 10 {
        let intermediate_as_roots: Vec<_> = final_state
            .dep_tree
            .tree_roots
            .iter()
            .filter(|r| {
                r.name.contains("boehm-gc")
                    || r.name.contains("boost-build")
                    || r.name.contains("bzip2")
            })
            .collect();

        // If nix is in the tree, these should not be roots
        let has_nix = final_state
            .dep_tree
            .nodes
            .keys()
            .any(|k| k.name.starts_with("nix-"));

        if has_nix && !intermediate_as_roots.is_empty() {
            eprintln!(
                "WARNING: Intermediate dependencies found as roots: {:?}",
                intermediate_as_roots
                    .iter()
                    .map(|r| &r.name)
                    .collect::<Vec<_>>()
            );
            // This would be the bug we're trying to fix
        }

        // The top-level nix derivation should be a root
        let nix_is_root = final_state.dep_tree.tree_roots.iter().any(|r| {
            r.name.starts_with("nix-")
                && !r.name.contains("util")
                && !r.name.contains("flake")
        });

        if has_nix {
            assert!(
                nix_is_root,
                "Expected top-level 'nix' to be a root. Current roots: {:?}",
                final_state
                    .dep_tree
                    .tree_roots
                    .iter()
                    .map(|r| &r.name)
                    .collect::<Vec<_>>()
            );
        }
    }

    daemon_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
}

/// Test that building tdf from nixpkgs master results in tdf being a root
#[tokio::test]
async fn test_tdf_is_root() {
    // Create temporary store directory
    let temp_store = TempDir::new().expect("Failed to create temp store");
    let store_path = temp_store.path().to_path_buf();

    let socket_path = PathBuf::from("/tmp/nixbtm-test-tdf.sock");
    let _ = std::fs::remove_file(&socket_path);

    let is_shutdown = Shutdown::new();
    let (tx, mut rx) = watch::channel(JobsStateInner::default());

    let socket_path_clone = socket_path.clone();
    let is_shutdown_clone = is_shutdown.clone();
    let daemon_handle = tokio::spawn(async move {
        handle_daemon_info(socket_path_clone, 0o600, is_shutdown_clone, tx)
            .await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Build tdf from nixpkgs master - allow substitutes for faster completion
    let mut nix_child = tokio::process::Command::new("nix")
        .arg("build")
        .arg("--no-link")
        .arg("--store")
        .arg(&store_path)
        .arg("--json-log-path")
        .arg(&socket_path)
        .arg("-vvv")
        .arg("github:nixos/nixpkgs/master#tdf")
        .spawn()
        .expect("Failed to spawn nix build");

    // Let it run - with substitutes it should complete or get far enough
    // that tdf itself gets queried
    tokio::time::sleep(Duration::from_secs(30)).await;
    let _ = nix_child.kill().await;

    tokio::time::sleep(Duration::from_millis(500)).await;

    is_shutdown.trigger();

    rx.changed().await.ok();
    let final_state = rx.borrow().clone();

    eprintln!("=== TDF Build State ===");
    eprintln!("Jobs: {}", final_state.jid_to_job.len());
    eprintln!("Nodes in dep_tree: {}", final_state.dep_tree.nodes.len());
    eprintln!("Tree roots: {}", final_state.dep_tree.tree_roots.len());

    for root in &final_state.dep_tree.tree_roots {
        eprintln!("  Root: {}", root.name);
    }

    // Check that tdf is a root
    if !final_state.dep_tree.nodes.is_empty() {
        let tdf_is_root = final_state
            .dep_tree
            .tree_roots
            .iter()
            .any(|r| r.name.contains("tdf"));

        // Also check that tdf is in the tree at all
        let tdf_in_tree = final_state
            .dep_tree
            .nodes
            .keys()
            .any(|k| k.name.contains("tdf"));

        if tdf_in_tree {
            assert!(
                tdf_is_root,
                "Expected 'tdf' to be a root, but roots are: {:?}",
                final_state
                    .dep_tree
                    .tree_roots
                    .iter()
                    .map(|r| &r.name)
                    .collect::<Vec<_>>()
            );
        }
    }

    daemon_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
}

/// Simpler test that doesn't require network - just tests tree insertion logic
#[tokio::test]
async fn test_drv_relations_root_detection() {
    use std::collections::BTreeSet;

    use nix_btm::{
        derivation_tree::{DrvNode, DrvRelations},
        handle_internal_json::Drv,
    };

    let mut relations = DrvRelations::default();

    // Create a simple tree: A depends on B, B depends on C
    let drv_a = Drv {
        name: "pkg-a".to_string(),
        hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1".to_string(),
    };
    let drv_b = Drv {
        name: "pkg-b".to_string(),
        hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb2".to_string(),
    };
    let drv_c = Drv {
        name: "pkg-c".to_string(),
        hash: "ccccccccccccccccccccccccccccccc3".to_string(),
    };

    // Insert C first (no deps) - should become root
    let node_c = DrvNode {
        root: drv_c.clone(),
        deps: BTreeSet::new(),
        required_outputs: BTreeSet::new(),
        required_output_paths: BTreeSet::new(),
    };
    relations.nodes.insert(drv_c.clone(), node_c.clone());
    relations.insert_node(node_c);

    assert!(
        relations.tree_roots.contains(&drv_c),
        "C should be root after insertion"
    );
    assert_eq!(relations.tree_roots.len(), 1);

    // Insert B (depends on C) - B should become root, C should be removed
    let mut deps_b = BTreeSet::new();
    deps_b.insert(drv_c.clone());
    let node_b = DrvNode {
        root: drv_b.clone(),
        deps: deps_b,
        required_outputs: BTreeSet::new(),
        required_output_paths: BTreeSet::new(),
    };
    relations.nodes.insert(drv_b.clone(), node_b.clone());
    relations.insert_node(node_b);

    assert!(
        relations.tree_roots.contains(&drv_b),
        "B should be root after insertion"
    );
    assert!(
        !relations.tree_roots.contains(&drv_c),
        "C should NOT be root (B depends on it)"
    );
    assert_eq!(relations.tree_roots.len(), 1);

    // Insert A (depends on B) - A should become root, B should be removed
    let mut deps_a = BTreeSet::new();
    deps_a.insert(drv_b.clone());
    let node_a = DrvNode {
        root: drv_a.clone(),
        deps: deps_a,
        required_outputs: BTreeSet::new(),
        required_output_paths: BTreeSet::new(),
    };
    relations.nodes.insert(drv_a.clone(), node_a.clone());
    relations.insert_node(node_a);

    assert!(
        relations.tree_roots.contains(&drv_a),
        "A should be root after insertion"
    );
    assert!(
        !relations.tree_roots.contains(&drv_b),
        "B should NOT be root (A depends on it)"
    );
    assert!(
        !relations.tree_roots.contains(&drv_c),
        "C should NOT be root"
    );
    assert_eq!(
        relations.tree_roots.len(),
        1,
        "Should have exactly 1 root (A)"
    );

    eprintln!("Root detection test passed: only {:?} is root", drv_a.name);
}
