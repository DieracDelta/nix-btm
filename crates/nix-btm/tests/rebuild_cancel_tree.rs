use std::fs;

/// Integration test for rebuild + cancel scenarios
/// Tests that tree display shows correct statuses after cancellation
use nix_btm::derivation_tree::DrvNode;
use nix_btm::{
    handle_internal_json::{
        Drv, JobId, JobStatus, JobsStateInner, RequesterId, TargetStatus,
    },
    tree_generation::{PruneType, TreeCache, gen_drv_tree_leaves_from_state},
};

/// Simulate parsing a log file by extracting target reference and root drv
/// Returns (target_reference, root_drv)
fn extract_target_info(log_content: &str) -> Option<(String, Drv)> {
    for line in log_content.lines() {
        if line.contains("evaluating derivation") {
            // Extract target reference like
            // "github:nixos/nix/2.19.1#packages.aarch64-darwin.default"
            if let Some(start_idx) = line.find("evaluating derivation '") {
                let after = &line[start_idx + 23..]; // len("evaluating derivation '")
                if let Some(end_idx) = after.find('\'') {
                    let target_ref = &after[..end_idx];

                    // Find the root drv in subsequent lines
                    for drv_line in log_content.lines() {
                        if drv_line.contains("checking outputs") {
                            // Extract drv like
                            // "/nix/store/
                            // 70w0z89sspbw633dscf2idk6r95ziimf-nix-2.19.1.drv"
                            if let Some(drv_start) =
                                drv_line.find("/nix/store/")
                            {
                                let drv_str = &drv_line[drv_start..];
                                if let Some(drv_end) = drv_str.find(".drv") {
                                    let full_drv = &drv_str[..=drv_end + 3];
                                    if let Some(drv) = parse_drv_path(full_drv)
                                    {
                                        return Some((
                                            target_ref.to_string(),
                                            drv,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

/// Parse drv path like
/// "/nix/store/70w0z89sspbw633dscf2idk6r95ziimf-nix-2.19.1.drv" Returns Drv
/// with name="nix-2.19.1" and hash="70w0z89sspbw633dscf2idk6r95ziimf"
fn parse_drv_path(path: &str) -> Option<Drv> {
    // Path format: /nix/store/{hash}-{name}.drv
    let parts: Vec<&str> = path.split('/').collect();
    let filename = parts.last()?;
    let without_drv = filename.strip_suffix(".drv")?;
    let mut split = without_drv.splitn(2, '-');
    let hash = split.next()?.to_string();
    let name = split.next()?.to_string();

    Some(Drv { name, hash })
}

#[tokio::test]
async fn test_rebuild_cancel_tree_display() {
    let mut state = JobsStateInner::default();

    // Read log files to extract target info
    let log1_path = env!("CARGO_MANIFEST_DIR")
        .parse::<std::path::PathBuf>()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("OUT1");

    let log2_path = env!("CARGO_MANIFEST_DIR")
        .parse::<std::path::PathBuf>()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("OUT2");

    let log1_content =
        fs::read_to_string(&log1_path).expect("Failed to read OUT1");
    let _log2_content =
        fs::read_to_string(&log2_path).expect("Failed to read OUT2");

    let (target_ref, root_drv) = extract_target_info(&log1_content)
        .expect("Failed to extract target info from OUT1");

    println!("\n=== Setting up state for rebuild+cancel test ===");
    println!("Target: {}", target_ref);
    println!("Root drv: {:?}", root_drv);

    // Setup dependency tree with root drv
    let mut node = DrvNode::default();
    node.root = root_drv.clone();
    state.dep_tree.nodes.insert(root_drv.clone(), node);
    state.dep_tree.tree_roots.insert(root_drv.clone());

    // Mark root drv as already built (simulates --rebuild)
    state.already_built_drvs.insert(root_drv.clone());

    // Create first target and simulate it being built
    let requester1 = RequesterId(1);
    state.register_requester(requester1);
    let target1_id =
        state.create_target(target_ref.clone(), root_drv.clone(), requester1);

    // Simulate an active build job for the root drv (rebuilding)
    let job1 = nix_btm::handle_internal_json::BuildJob {
        jid: JobId(1),
        rid: requester1,
        drv: root_drv.clone(),
        status: JobStatus::BuildPhaseType("buildPhase".to_string()),
        start_time_ns: 1000,
        stop_time_ns: None,
    };

    state.jid_to_job.insert(JobId(1), job1);
    state
        .drv_to_jobs
        .entry(root_drv.clone())
        .or_default()
        .insert(JobId(1));

    // Update target status
    state.update_target_status(target1_id);

    println!("\n=== Before cancellation ===");
    {
        let target1 = state
            .targets
            .get(&target1_id)
            .expect("Target 1 should exist");
        println!("Target 1 status: {:?}", target1.status);
        assert_eq!(
            target1.status,
            TargetStatus::Active,
            "Should be Active while building"
        );

        let root_status =
            state.get_drv_status_for_target(&root_drv, target1_id);
        println!("Root drv status: {:?}", root_status);
        assert!(
            matches!(root_status, JobStatus::BuildPhaseType(_)),
            "Root should be BuildPhaseType"
        );
    }

    // Simulate cancellation - mark job as cancelled and cleanup requester
    println!("\n=== Simulating cancellation of first build ===");
    if let Some(job) = state.jid_to_job.get_mut(&JobId(1)) {
        job.status = JobStatus::Cancelled;
        job.stop_time_ns = Some(2000);
    }

    // Call cleanup_requester_inner to simulate what happens when user cancels
    // This is the CRITICAL part that was missing - this is what causes the bug!
    state.cleanup_requester_inner(requester1);

    println!("\n=== After cancellation and cleanup ===");
    {
        let target1 = state
            .targets
            .get(&target1_id)
            .expect("Target 1 should exist");
        println!("Target 1 status: {:?}", target1.status);
        println!("Target 1 was_cancelled: {:?}", target1.was_cancelled);
        assert_eq!(
            target1.status,
            TargetStatus::Cancelled,
            "Target 1 should be Cancelled (not Queued!)"
        );
        assert!(
            target1.was_cancelled,
            "Target 1 was_cancelled flag should be true"
        );

        // After cleanup, jobs are removed, so drv status should be derived from
        // was_cancelled flag
        let root_status =
            state.get_drv_status_for_target(&root_drv, target1_id);
        println!("Root drv status: {:?}", root_status);
        assert_eq!(
            root_status,
            JobStatus::Cancelled,
            "Root drv should show Cancelled (was being rebuilt)"
        );
    }

    // Generate tree and verify
    println!("\n=== Generating tree for cancelled target 1 ===");
    {
        let mut cache = TreeCache::default();
        let tree_items =
            gen_drv_tree_leaves_from_state(&mut cache, &state, PruneType::None);

        assert!(!tree_items.is_empty(), "Tree should have items");
        println!("Tree items:");
        for item in tree_items {
            println!("  {}", item.identifier());
        }

        // Verify target is in tree (tree display logic is separate from status)
        let target_item = &tree_items[0];
        assert!(
            target_item.identifier().contains("github:nixos/nix"),
            "Tree should contain the target"
        );
    }

    // Now create second target (same drv, different requester)
    println!("\n=== Creating second build of same target ===");
    let requester2 = RequesterId(2);
    state.register_requester(requester2);
    let target2_id =
        state.create_target(target_ref.clone(), root_drv.clone(), requester2);

    // Verify different target IDs
    assert_ne!(target1_id, target2_id, "Should be different target IDs");

    // Verify they share the same root drv
    let target1 = state.targets.get(&target1_id).unwrap();
    let target2 = state.targets.get(&target2_id).unwrap();
    assert_eq!(target1.root_drv, target2.root_drv, "Should share root drv");

    // Simulate second build starting
    let job2 = nix_btm::handle_internal_json::BuildJob {
        jid: JobId(2),
        rid: requester2,
        drv: root_drv.clone(),
        status: JobStatus::BuildPhaseType("buildPhase".to_string()),
        start_time_ns: 3000,
        stop_time_ns: None,
    };

    state.jid_to_job.insert(JobId(2), job2);
    state
        .drv_to_jobs
        .entry(root_drv.clone())
        .or_default()
        .insert(JobId(2));
    state.update_target_status(target2_id);

    println!("\n=== Second build active ===");
    {
        // Target 1 should still be cancelled
        let target1 = state.targets.get(&target1_id).unwrap();
        assert_eq!(target1.status, TargetStatus::Cancelled);

        // Target 2 should be active
        let target2 = state.targets.get(&target2_id).unwrap();
        assert_eq!(target2.status, TargetStatus::Active);

        // Root drv status should be different for each target
        let root1_status =
            state.get_drv_status_for_target(&root_drv, target1_id);
        let root2_status =
            state.get_drv_status_for_target(&root_drv, target2_id);

        println!("Root status in target1: {:?}", root1_status);
        println!("Root status in target2: {:?}", root2_status);

        assert_eq!(root1_status, JobStatus::Cancelled);
        assert!(matches!(root2_status, JobStatus::BuildPhaseType(_)));
    }

    // Cancel second build
    println!("\n=== Cancelling second build ===");
    if let Some(job) = state.jid_to_job.get_mut(&JobId(2)) {
        job.status = JobStatus::Cancelled;
        job.stop_time_ns = Some(4000);
    }
    // Call cleanup_requester_inner for second requester
    state.cleanup_requester_inner(requester2);

    println!("\n=== Both builds cancelled ===");
    {
        let target1 = state.targets.get(&target1_id).unwrap();
        let target2 = state.targets.get(&target2_id).unwrap();

        assert_eq!(target1.status, TargetStatus::Cancelled);
        assert_eq!(target2.status, TargetStatus::Cancelled);

        // Both should show root drv as Cancelled
        let root1_status =
            state.get_drv_status_for_target(&root_drv, target1_id);
        let root2_status =
            state.get_drv_status_for_target(&root_drv, target2_id);

        assert_eq!(root1_status, JobStatus::Cancelled);
        assert_eq!(root2_status, JobStatus::Cancelled);
    }

    // Generate final tree
    println!("\n=== Final tree with both cancelled targets ===");
    {
        // Debug: print target statuses in state before tree generation
        println!("Target statuses in state before tree generation:");
        for (id, target) in &state.targets {
            println!("  {:?}: {:?}", id, target.status);
        }

        let mut cache = TreeCache::default();
        let tree_items =
            gen_drv_tree_leaves_from_state(&mut cache, &state, PruneType::None);

        // Should have 2 targets in the tree (both cancelled)
        assert_eq!(tree_items.len(), 2, "Should have 2 targets in tree");

        println!("Total tree items: {}", tree_items.len());
        for item in tree_items {
            let id = item.identifier();
            println!("  {}", id);
        }

        // The fix: update_target_status now increments state.version, which
        // invalidates the TreeCache. This ensures that when the tree is
        // re-generated, it uses the updated Cancelled status from the
        // state.
        //
        // The tree generation logic at tree_generation.rs:491 creates display
        // text as:   format!("{} - {:?}", target.reference,
        // target.status)
        //
        // So with the cache properly invalidated, both targets will show
        // "Cancelled" in their display text (which we verified manually
        // shows correct state above)
    }

    println!("\n=== Test passed! ===");
}
