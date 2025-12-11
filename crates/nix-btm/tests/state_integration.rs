use nix_btm::{
    derivation_tree::DrvNode,
    handle_internal_json::{
        Drv, JobId, JobStatus, JobsStateInner, RequesterId,
    },
    tree_generation::{PruneType, gen_drv_tree_leaves_from_state},
};

/// Test that we correctly parse the target from Nix logs
#[test]
fn test_target_detection() {
    let _log_line = r#"{"action":"start","id":408262411288576,"level":4,"parent":0,"text":"evaluating derivation 'github:nixos/nixpkgs/master#haskellPackages.hoogle'","type":0}"#;

    // This simulates what handle_line does when it sees "evaluating derivation"
    let text = "evaluating derivation \
                'github:nixos/nixpkgs/master#haskellPackages.hoogle'";
    let target = text
        .strip_prefix("evaluating derivation '")
        .and_then(|s| s.strip_suffix("'"))
        .expect("Should extract target");

    assert_eq!(target, "github:nixos/nixpkgs/master#haskellPackages.hoogle");
}

/// Test that BuildTarget correctly stores target info and transitive closure
#[test]
fn test_build_target_structure() {
    let mut state = JobsStateInner::default();

    // Create a simple dependency tree: hoogle -> aeson -> text
    let hoogle = Drv {
        name: "hoogle-unstable-2024-07-29".to_string(),
        hash: "9zxhh5ddz96xr84k30x35iizfzp1mzj3".to_string(),
    };
    let aeson = Drv {
        name: "aeson-2.2.3.0".to_string(),
        hash: "10s78v8l8cw33ahkcx4pjb0zdpb8yspr".to_string(),
    };
    let text = Drv {
        name: "text-short-0.1.6".to_string(),
        hash: "lkn4xcsz9c51k7zpmsiw5j5mbqr17cz0".to_string(),
    };

    // Build dependency tree
    let mut hoogle_node = DrvNode::default();
    hoogle_node.root = hoogle.clone();
    hoogle_node.deps.insert(aeson.clone());

    let mut aeson_node = DrvNode::default();
    aeson_node.root = aeson.clone();
    aeson_node.deps.insert(text.clone());

    let mut text_node = DrvNode::default();
    text_node.root = text.clone();

    state.dep_tree.nodes.insert(hoogle.clone(), hoogle_node);
    state.dep_tree.nodes.insert(aeson.clone(), aeson_node);
    state.dep_tree.nodes.insert(text.clone(), text_node);
    state.dep_tree.tree_roots.insert(hoogle.clone());

    // Create BuildTarget for hoogle
    state.register_requester(RequesterId(1));
    let target_id = state.create_target(
        "github:nixos/nixpkgs/master#haskellPackages.hoogle".to_string(),
        hoogle.clone(),
        RequesterId(1),
    );

    // Verify target was created correctly
    let target = state.targets.get(&target_id).expect("Target should exist");
    assert_eq!(
        target.reference,
        "github:nixos/nixpkgs/master#haskellPackages.hoogle"
    );
    assert_eq!(target.root_drv, hoogle);
    assert_eq!(target.requester_id, RequesterId(1));

    // Verify transitive closure includes all dependencies
    assert!(
        target.transitive_closure.contains(&hoogle),
        "Should contain root"
    );
    assert!(
        target.transitive_closure.contains(&aeson),
        "Should contain aeson dependency"
    );
    assert!(
        target.transitive_closure.contains(&text),
        "Should contain text dependency"
    );
    assert_eq!(
        target.transitive_closure.len(),
        3,
        "Should have exactly 3 drvs"
    );

    // Verify reverse index
    assert!(
        state
            .drv_to_targets
            .get(&hoogle)
            .unwrap()
            .contains(&target_id)
    );
    assert!(
        state
            .drv_to_targets
            .get(&aeson)
            .unwrap()
            .contains(&target_id)
    );
    assert!(
        state
            .drv_to_targets
            .get(&text)
            .unwrap()
            .contains(&target_id)
    );
}

/// Test that job status correctly affects target status
#[test]
fn test_target_status_from_jobs() {
    let mut state = JobsStateInner::default();

    // Create simple tree
    let drv = Drv {
        name: "aeson-2.2.3.0".to_string(),
        hash: "10s78v8l8cw33ahkcx4pjb0zdpb8yspr".to_string(),
    };

    let mut node = DrvNode::default();
    node.root = drv.clone();
    state.dep_tree.nodes.insert(drv.clone(), node);
    state.dep_tree.tree_roots.insert(drv.clone());

    // Create target
    state.register_requester(RequesterId(1));
    let target_id = state.create_target(
        "nixpkgs#aeson".to_string(),
        drv.clone(),
        RequesterId(1),
    );

    // Initially should be Evaluating (no jobs)
    let target = state.targets.get(&target_id).unwrap();
    assert_eq!(
        target.status,
        nix_btm::handle_internal_json::TargetStatus::Evaluating
    );

    // Add a downloading job
    let job = nix_btm::handle_internal_json::BuildJob {
        jid: JobId(1),
        rid: RequesterId(1),
        drv: drv.clone(),
        status: JobStatus::Downloading {
            url: "https://cache.nixos.org/aeson".to_string(),
            done_bytes: 1024,
            total_bytes: 4096,
        },
        start_time_ns: 1000,
        stop_time_ns: None,
    };

    state.jid_to_job.insert(JobId(1), job);
    state
        .drv_to_jobs
        .entry(drv.clone())
        .or_default()
        .insert(JobId(1));

    // Update target status
    state.update_target_status(target_id);

    // Should now be Active (job is downloading)
    let target = state.targets.get(&target_id).unwrap();
    assert_eq!(
        target.status,
        nix_btm::handle_internal_json::TargetStatus::Active
    );
}

/// Test that targets with all cached drvs are marked as Cached
#[test]
fn test_target_status_cached() {
    let mut state = JobsStateInner::default();

    let drv = Drv {
        name: "text-short-0.1.6".to_string(),
        hash: "lkn4xcsz9c51k7zpmsiw5j5mbqr17cz0".to_string(),
    };

    let mut node = DrvNode::default();
    node.root = drv.clone();
    state.dep_tree.nodes.insert(drv.clone(), node);
    state.dep_tree.tree_roots.insert(drv.clone());

    // Mark drv as already built
    state.already_built_drvs.insert(drv.clone());

    // Create target
    state.register_requester(RequesterId(1));
    let target_id = state.create_target(
        "nixpkgs#text-short".to_string(),
        drv.clone(),
        RequesterId(1),
    );

    // Update status
    state.update_target_status(target_id);

    // Should be Cached (all drvs already built, no jobs)
    let target = state.targets.get(&target_id).unwrap();
    assert_eq!(
        target.status,
        nix_btm::handle_internal_json::TargetStatus::Cached
    );
}

/// Test that per-target drv status works correctly (Option C semantics)
#[test]
fn test_per_target_drv_status() {
    let mut state = JobsStateInner::default();

    // Shared dependency
    let shared = Drv {
        name: "text-2.0".to_string(),
        hash: "abc123def456abc123def456abc12345".to_string(),
    };

    let target_a_drv = Drv {
        name: "aeson-2.2.3.0".to_string(),
        hash: "10s78v8l8cw33ahkcx4pjb0zdpb8yspr".to_string(),
    };

    let target_b_drv = Drv {
        name: "attoparsec-0.14.4".to_string(),
        hash: "l3ic0mra8r56drc1wlffbn2pxhgmchgi".to_string(),
    };

    // Build dependency trees
    let mut a_node = DrvNode::default();
    a_node.root = target_a_drv.clone();
    a_node.deps.insert(shared.clone());

    let mut b_node = DrvNode::default();
    b_node.root = target_b_drv.clone();
    b_node.deps.insert(shared.clone());

    let mut shared_node = DrvNode::default();
    shared_node.root = shared.clone();

    state.dep_tree.nodes.insert(target_a_drv.clone(), a_node);
    state.dep_tree.nodes.insert(target_b_drv.clone(), b_node);
    state.dep_tree.nodes.insert(shared.clone(), shared_node);
    state.dep_tree.tree_roots.insert(target_a_drv.clone());
    state.dep_tree.tree_roots.insert(target_b_drv.clone());

    // Create two targets
    state.register_requester(RequesterId(1));
    let target_a_id = state.create_target(
        "nixpkgs#aeson".to_string(),
        target_a_drv.clone(),
        RequesterId(1),
    );

    state.register_requester(RequesterId(2));
    let target_b_id = state.create_target(
        "nixpkgs#attoparsec".to_string(),
        target_b_drv.clone(),
        RequesterId(2),
    );

    // Add job for shared drv from requester 2 only
    let job = nix_btm::handle_internal_json::BuildJob {
        jid: JobId(1),
        rid: RequesterId(2),
        drv: shared.clone(),
        status: JobStatus::Downloading {
            url: "https://cache.nixos.org/text".to_string(),
            done_bytes: 500,
            total_bytes: 1000,
        },
        start_time_ns: 1000,
        stop_time_ns: None,
    };

    state.jid_to_job.insert(JobId(1), job);
    state
        .drv_to_jobs
        .entry(shared.clone())
        .or_default()
        .insert(JobId(1));

    // Get per-target status of shared drv
    let status_in_a = state.get_drv_status_for_target(&shared, target_a_id);
    let status_in_b = state.get_drv_status_for_target(&shared, target_b_id);

    // In target A (requester 1): no job, so should be Queued (in dep tree)
    assert_eq!(status_in_a, JobStatus::Queued);

    // In target B (requester 2): has downloading job
    assert!(matches!(status_in_b, JobStatus::Downloading { .. }));
}

/// Test that tree generation includes targets
#[test]
fn test_tree_generation_with_targets() {
    let mut state = JobsStateInner::default();

    let drv = Drv {
        name: "hoogle-unstable-2024-07-29".to_string(),
        hash: "9zxhh5ddz96xr84k30x35iizfzp1mzj3".to_string(),
    };

    let mut node = DrvNode::default();
    node.root = drv.clone();
    state.dep_tree.nodes.insert(drv.clone(), node);
    state.dep_tree.tree_roots.insert(drv.clone());

    // Create target
    state.register_requester(RequesterId(1));
    state.create_target(
        "github:nixos/nixpkgs/master#haskellPackages.hoogle".to_string(),
        drv.clone(),
        RequesterId(1),
    );

    // Generate tree with no pruning
    let mut cache = nix_btm::tree_generation::TreeCache::default();
    let tree =
        gen_drv_tree_leaves_from_state(&mut cache, &state, PruneType::None);

    // Should have at least one root (the target)
    assert!(!tree.is_empty(), "Tree should not be empty");

    // The root should be the target reference
    let root = &tree[0];
    assert!(
        root.identifier().contains("haskellPackages.hoogle"),
        "Root should contain target reference"
    );
}

/// Test that cancelled targets update status correctly
#[test]
fn test_target_cancellation() {
    let mut state = JobsStateInner::default();

    let drv = Drv {
        name: "aeson-2.2.3.0".to_string(),
        hash: "10s78v8l8cw33ahkcx4pjb0zdpb8yspr".to_string(),
    };

    let mut node = DrvNode::default();
    node.root = drv.clone();
    state.dep_tree.nodes.insert(drv.clone(), node);
    state.dep_tree.tree_roots.insert(drv.clone());

    state.register_requester(RequesterId(1));
    let target_id = state.create_target(
        "nixpkgs#aeson".to_string(),
        drv.clone(),
        RequesterId(1),
    );

    // Add a job that gets cancelled
    let job = nix_btm::handle_internal_json::BuildJob {
        jid: JobId(1),
        rid: RequesterId(1),
        drv: drv.clone(),
        status: JobStatus::Cancelled,
        start_time_ns: 1000,
        stop_time_ns: Some(2000),
    };

    state.jid_to_job.insert(JobId(1), job);
    state
        .drv_to_jobs
        .entry(drv.clone())
        .or_default()
        .insert(JobId(1));

    // Update target status
    state.update_target_status(target_id);

    // Should be Cancelled
    let target = state.targets.get(&target_id).unwrap();
    assert_eq!(
        target.status,
        nix_btm::handle_internal_json::TargetStatus::Cancelled
    );
}

/// Test multiple targets can share dependencies
#[test]
fn test_shared_dependencies_between_targets() {
    let mut state = JobsStateInner::default();

    // Shared dep
    let text = Drv {
        name: "text-2.0".to_string(),
        hash: "abc123def456abc123def456abc12345".to_string(),
    };

    // Target A and B both depend on text
    let aeson = Drv {
        name: "aeson-2.2.3.0".to_string(),
        hash: "10s78v8l8cw33ahkcx4pjb0zdpb8yspr".to_string(),
    };

    let attoparsec = Drv {
        name: "attoparsec-0.14.4".to_string(),
        hash: "l3ic0mra8r56drc1wlffbn2pxhgmchgi".to_string(),
    };

    // Setup trees
    let mut aeson_node = DrvNode::default();
    aeson_node.deps.insert(text.clone());
    let mut attoparsec_node = DrvNode::default();
    attoparsec_node.deps.insert(text.clone());
    let text_node = DrvNode::default();

    state.dep_tree.nodes.insert(aeson.clone(), aeson_node);
    state
        .dep_tree
        .nodes
        .insert(attoparsec.clone(), attoparsec_node);
    state.dep_tree.nodes.insert(text.clone(), text_node);
    state.dep_tree.tree_roots.insert(aeson.clone());
    state.dep_tree.tree_roots.insert(attoparsec.clone());

    // Create two targets
    state.register_requester(RequesterId(1));
    let target_a = state.create_target(
        "nixpkgs#aeson".to_string(),
        aeson.clone(),
        RequesterId(1),
    );

    state.register_requester(RequesterId(2));
    let target_b = state.create_target(
        "nixpkgs#attoparsec".to_string(),
        attoparsec.clone(),
        RequesterId(2),
    );

    // Verify text drv appears in both targets
    let a = state.targets.get(&target_a).unwrap();
    let b = state.targets.get(&target_b).unwrap();

    assert!(
        a.transitive_closure.contains(&text),
        "Target A should include text"
    );
    assert!(
        b.transitive_closure.contains(&text),
        "Target B should include text"
    );

    // Verify reverse index shows text belongs to both targets
    let text_targets = state.drv_to_targets.get(&text).unwrap();
    assert!(
        text_targets.contains(&target_a),
        "text should map to target A"
    );
    assert!(
        text_targets.contains(&target_b),
        "text should map to target B"
    );
    assert_eq!(
        text_targets.len(),
        2,
        "text should belong to exactly 2 targets"
    );
}
