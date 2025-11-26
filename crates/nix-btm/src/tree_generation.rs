use std::collections::{HashMap, HashSet, VecDeque};

use tracing::{error, info};
use tui_tree_widget::{TreeItem, TreeState};

use crate::handle_internal_json::{Drv, JobStatus, JobsStateInner};

/// Cache for tree generation to avoid rebuilding identical trees
#[derive(Debug)]
pub struct TreeCache {
    cached_tree: Vec<TreeItem<'static, String>>,
    cached_state_version: u64,
    cached_prune_mode: PruneType,
}

impl Default for TreeCache {
    fn default() -> Self {
        Self {
            cached_tree: Vec::new(),
            cached_state_version: 0,
            cached_prune_mode: PruneType::Normal,
        }
    }
}

impl TreeCache {
    /// Get the cached tree or rebuild if necessary
    /// Returns a reference to the cached tree (avoids cloning large trees)
    pub fn get_or_build<'a>(
        &'a mut self,
        state: &JobsStateInner,
        prune_mode: PruneType,
    ) -> &'a Vec<TreeItem<'static, String>> {
        // Check if cache is valid
        let needs_rebuild = self.cached_state_version != state.version
            || self.cached_prune_mode != prune_mode;

        if needs_rebuild {
            // Rebuild the tree
            self.cached_tree = gen_drv_tree_leaves_from_state_uncached(state, prune_mode);
            self.cached_state_version = state.version;
            self.cached_prune_mode = prune_mode;
        }

        &self.cached_tree
    }
}

#[derive(Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum PruneType {
    None,
    Aggressive,
    #[default]
    Normal,
}

impl PruneType {
    pub fn increment(self) -> Self {
        match self {
            PruneType::None => PruneType::Normal,
            PruneType::Normal => PruneType::Aggressive,
            PruneType::Aggressive => PruneType::None,
        }
    }
}

fn reachable_active_leaves(
    d: &Drv,
    state: &JobsStateInner,
    active: &HashSet<Drv>,
    memo: &mut HashMap<Drv, HashSet<Drv>>,
) -> HashSet<Drv> {
    if let Some(s) = memo.get(d) {
        return s.clone();
    }
    if state.get_status(d).is_active() {
        let mut s = HashSet::new();
        s.insert(d.clone());
        memo.insert(d.clone(), s.clone());
        return s;
    }
    let mut out = HashSet::new();
    if let Some(node) = state.dep_tree.nodes.get(d) {
        for child in &node.deps {
            if active.contains(child) {
                let s = reachable_active_leaves(child, state, active, memo);
                out.extend(s);
            }
        }
    }
    memo.insert(d.clone(), out.clone());
    out
}

/// Aggressive mode: collapse wrappers to the first visible node.
fn collapse_to_visible_owned(
    d: &Drv,
    state: &JobsStateInner,
    active: &HashSet<Drv>,
    leaves_memo: &mut HashMap<Drv, HashSet<Drv>>,
) -> Drv {
    if state.get_status(d).is_active() {
        return d.clone();
    }
    let leaves = reachable_active_leaves(d, state, active, leaves_memo);
    match leaves.len() {
        0 => d.clone(),                          // dead end
        1 => leaves.into_iter().next().unwrap(), // jump to leaf
        _ => d.clone(),                          // branch point
    }
}

pub fn explore_root(
    root: &mut TreeItem<'_, String>,
    state: &JobsStateInner,
    root_drv: &Drv,
    prune: PruneType,
    active_closure: Option<&HashSet<Drv>>,
    target_id: Option<crate::handle_internal_json::BuildTargetId>,
) {
    // reshuffle if mismatched args (shouldn't be possible)
    let active = if prune == PruneType::None {
        None
    } else {
        active_closure
    };

    // Helper closures for target-aware status lookups
    let get_status = |drv: &Drv| -> crate::handle_internal_json::JobStatus {
        if let Some(tid) = target_id {
            state.get_drv_status_for_target(drv, tid)
        } else {
            state.get_status(drv)
        }
    };

    let make_description = |drv: &Drv| -> String {
        if let Some(tid) = target_id {
            state.make_tree_description_for_target(drv, tid)
        } else {
            state.make_tree_description(drv)
        }
    };

    if let Some(ac) = active
        && !ac.contains(root_drv)
    {
        return; // root not on a path to any active leaf
    }

    let mut printed_leaves: HashSet<Drv> = HashSet::new();
    let mut leaves_memo: HashMap<Drv, HashSet<Drv>> = HashMap::new();

    let mut stack: Vec<(Drv, Vec<usize>)> =
        vec![(root_drv.clone(), Vec::new())];
    let mut seen_parents: HashSet<Drv> = HashSet::new();

    while let Some((parent_drv, path)) = stack.pop() {
        if !seen_parents.insert(parent_drv.clone()) {
            continue;
        }

        if let Some(children) =
            state.dep_tree.nodes.get(&parent_drv).map(|n| &n.deps)
        {
            let mut ui = &mut *root;
            for &i in &path {
                ui = ui.child_mut(i).expect("UI path out of sync");
            }

            let mut added_ids: HashSet<String> = HashSet::new(); // per-parent dedupe
            let mut idx = 0;

            let mut kept_children: Vec<&Drv> = Vec::new();
            if let (PruneType::Normal, Some(ac)) = (prune, active) {
                let mut assigned_here: HashSet<Drv> = HashSet::new();
                for child in children {
                    if !ac.contains(child) {
                        continue;
                    }
                    if get_status(child).is_active() {
                        if !printed_leaves.contains(child) {
                            kept_children.push(child);
                            assigned_here.insert(child.clone());
                        }
                        continue;
                    }
                    let mut leaves = reachable_active_leaves(
                        child,
                        state,
                        ac,
                        &mut leaves_memo,
                    );
                    leaves.retain(|l| !printed_leaves.contains(l));
                    let contributes_new =
                        leaves.iter().any(|l| !assigned_here.contains(l));
                    if contributes_new {
                        assigned_here.extend(leaves);
                        kept_children.push(child);
                    }
                }
            }

            let iter: Box<dyn Iterator<Item = &Drv> + '_> =
                match (prune, active) {
                    (PruneType::Normal, Some(_)) => {
                        Box::new(kept_children.into_iter())
                    }
                    _ => Box::new(children.iter()),
                };

            for child in iter {
                if let (PruneType::Aggressive, Some(ac)) = (prune, active)
                    && !ac.contains(child)
                {
                    continue;
                }

                let (to_render, push_drv): (Option<Drv>, Option<Drv>) =
                    match (prune, active) {
                        (PruneType::None, _) => {
                            (Some(child.clone()), Some(child.clone()))
                        }

                        (PruneType::Normal, Some(ac)) => {
                            if get_status(child).is_active() {
                                if !printed_leaves.insert(child.clone()) {
                                    (None, None)
                                } else {
                                    (Some(child.clone()), None)
                                }
                            } else {
                                let mut leaves = reachable_active_leaves(
                                    child,
                                    state,
                                    ac,
                                    &mut leaves_memo,
                                );
                                leaves.retain(|l| !printed_leaves.contains(l));
                                if leaves.is_empty() {
                                    (None, None)
                                } else {
                                    (Some(child.clone()), Some(child.clone()))
                                }
                            }
                        }

                        (PruneType::Aggressive, Some(ac)) => {
                            let vis = collapse_to_visible_owned(
                                child,
                                state,
                                ac,
                                &mut leaves_memo,
                            );
                            if get_status(&vis).is_active() {
                                if !printed_leaves.insert(vis.clone()) {
                                    (None, None)
                                } else {
                                    (Some(vis.clone()), None)
                                }
                            } else {
                                (Some(vis.clone()), Some(vis))
                            }
                        }

                        (PruneType::Normal | PruneType::Aggressive, None) => {
                            (Some(child.clone()), Some(child.clone()))
                        }
                    };

                if let Some(vis) = to_render {
                    let base_ident = vis.to_string();
                    if !added_ids.insert(base_ident.clone()) {
                        continue;
                    }

                    // Use path-based identifier to ensure uniqueness across the
                    // entire tree Format: "path_idx0/
                    // path_idx1/.../drv_string"
                    let ident = if path.is_empty() {
                        base_ident.clone()
                    } else {
                        let path_str = path
                            .iter()
                            .map(|i| i.to_string())
                            .collect::<Vec<_>>()
                            .join("/");
                        format!("{}/{}", path_str, base_ident)
                    };

                    let node = TreeItem::new(
                        ident,
                        make_description(&vis),
                        vec![],
                    )
                    .expect("TreeItem::new failed");

                    if ui.add_child(node).is_ok() {
                        if let Some(next) = push_drv {
                            let mut next_path = path.clone();
                            next_path.push(idx);
                            stack.push((next, next_path));
                        }
                        idx += 1;
                    } else {
                        tracing::warn!(
                            "duplicate child under {:?}, skipped",
                            parent_drv
                        );
                    }
                }
            }
        }
    }
}

// iterate through tree roots
// print drv using the tree roots
// for each drv, look up and see if there are any build jobs going on. If
// there are, then you can use that to deduce the status. If there are
// none, you take the L and say unused

/// Internal function that actually builds the tree (no caching)
fn gen_drv_tree_leaves_from_state_uncached(
    state: &JobsStateInner,
    do_prune: PruneType,
) -> Vec<TreeItem<'static, String>> {
    // Debug: log tree roots and active closure
    error!("=== PRUNE DEBUG ===");
    error!("Prune mode: {:?}", do_prune);
    error!("Tree roots ({}):", state.dep_tree.tree_roots.len());
    for root in &state.dep_tree.tree_roots {
        error!("  ROOT: {} - {}", root.name, root.hash);
    }
    error!("Total nodes in tree: {}", state.dep_tree.nodes.len());

    // Show active items
    let active_items: Vec<_> = state
        .dep_tree
        .nodes
        .keys()
        .filter(|d| state.get_status(d).is_active())
        .collect();
    error!("Active items ({}):", active_items.len());
    for d in &active_items {
        error!(
            "  ACTIVE: {} - {} - {}",
            d.name,
            d.hash,
            state.get_status(d)
        );
    }

    // For Aggressive mode: flat list of only active items
    if do_prune == PruneType::Aggressive {
        let mut items = vec![];
        for drv in state.dep_tree.nodes.keys() {
            if state.get_status(drv).is_active() {
                let item = TreeItem::new(
                    drv.to_string(),
                    state.make_tree_description(drv),
                    vec![],
                )
                .unwrap();
                items.push(item);
            }
        }
        // Also include jobs that might not be in dep_tree (synthetic drvs)
        for job in state.jid_to_job.values() {
            if job.status.is_active()
                && !state.dep_tree.nodes.contains_key(&job.drv)
            {
                let item = TreeItem::new(
                    job.drv.to_string(),
                    state.make_tree_description(&job.drv),
                    vec![],
                )
                .unwrap();
                items.push(item);
            }
        }
        info!("aggressive prune: {} active items", items.len());
        return items;
    }

    let active = if do_prune == PruneType::Normal {
        let ac = compute_active_closure(state);
        error!("Active closure ({}):", ac.len());
        for d in &ac {
            error!("  CLOSURE: {} - {}", d.name, d.hash);
        }
        Some(ac)
    } else {
        None
    };

    let mut roots = vec![];

    error!(
        "Total targets: {}, Total dep_tree roots: {}",
        state.targets.len(),
        state.dep_tree.tree_roots.len()
    );

    // Build list of all targets
    // Each target instance gets its own tree node (even if same reference/drv)
    // Include cancelled targets - they should still be visible
    let all_targets: Vec<_> = state.targets.values().collect();

    // Also find orphan roots (in dep_tree but not belonging to any target)
    let target_root_drvs: HashSet<_> =
        all_targets.iter().map(|t| &t.root_drv).collect();
    let mut orphan_roots: Vec<&Drv> = state
        .dep_tree
        .tree_roots
        .iter()
        .filter(|root| !target_root_drvs.contains(root))
        .collect();

    // For Normal pruning, filter orphan roots by active closure
    if let Some(ref ac) = active {
        orphan_roots.retain(|root| ac.contains(root));
    }

    // Create tree items for each target instance
    // Use target ID to ensure uniqueness even if multiple builds of same ref
    for target in &all_targets {
        // Skip if Normal pruning and root not in active closure
        if let Some(ref ac) = active {
            if !ac.contains(&target.root_drv) {
                error!("  SKIPPING target '{}' - not in closure", target.reference);
                continue;
            }
        }

        error!(
            "Creating tree node for target '{}' (ID: {:?}, root drv: {})",
            target.reference, target.id, target.root_drv.name
        );

        // Build tree for this target's root drv
        let a_root = &target.root_drv;
        error!("building tree node for {a_root}");

        let mut drv_node = TreeItem::new(
            a_root.clone().to_string(),
            state.make_tree_description_for_target(a_root, target.id),
            vec![],
        )
        .unwrap();

        explore_root(
            &mut drv_node,
            state,
            a_root,
            do_prune,
            active.as_ref(),
            Some(target.id),
        );

        // For Normal pruning, check if we should include this target
        // Use target-level status, not drv-level status
        let is_target_active = matches!(
            target.status,
            crate::handle_internal_json::TargetStatus::Active
                | crate::handle_internal_json::TargetStatus::Evaluating
        );
        let is_target_completed = matches!(
            target.status,
            crate::handle_internal_json::TargetStatus::Completed
                | crate::handle_internal_json::TargetStatus::Cached
        );

        if do_prune == PruneType::Normal
            && drv_node.children().is_empty()
            && !is_target_active
            && !is_target_completed
        {
            error!(
                "FILTERING OUT target '{}' - no children, not active, not \
                 completed",
                target.reference
            );
            continue;
        }

        // Create target node with drv as child
        // Use target ID in the identifier to ensure uniqueness
        let target_identifier = format!("{}:{:?}", target.reference, target.id);
        // Use target-level status instead of drv-level status
        let target_display = format!("{} - {:?}", target.reference, target.status);

        let target_node =
            TreeItem::new(target_identifier, target_display, vec![drv_node])
                .unwrap();
        roots.push(target_node);
    }

    // Add orphan roots (no target) directly
    for a_root in orphan_roots {
        error!("building tree node for {a_root}");
        let mut new_root = TreeItem::new(
            a_root.clone().to_string(),
            state.make_tree_description(a_root),
            vec![],
        )
        .unwrap();
        explore_root(&mut new_root, state, a_root, do_prune, active.as_ref(), None);

        // For Normal pruning, only include if root has children or is itself
        // active or completed
        let status = state.get_status(a_root);
        if do_prune == PruneType::Normal
            && new_root.children().is_empty()
            && !status.is_active()
            && !status.is_completed()
            && !matches!(status, JobStatus::AlreadyBuilt)
        {
            error!(
                "FILTERING OUT orphan root {} - no children, not active, not \
                 completed",
                a_root.name
            );
            continue;
        }

        roots.push(new_root);
    }

    info!("total number roots {} ", roots.len());

    roots
}

pub fn compute_active_closure(state: &JobsStateInner) -> HashSet<Drv> {
    // Build reverse adjacency: child -> parents
    let mut rev: HashMap<&Drv, Vec<&Drv>> = HashMap::new();
    for (parent, node) in &state.dep_tree.nodes {
        for child in &node.deps {
            rev.entry(child).or_default().push(parent);
        }
    }

    // Seed with all currently active nodes (by reference to avoid cloning)
    let mut q: VecDeque<&Drv> = state
        .dep_tree
        .nodes
        .keys()
        .filter(|d| state.get_status(d).is_active())
        .collect();

    // Mark visited (by reference), then clone once at the end
    let mut marked: HashSet<&Drv> = HashSet::new();

    while let Some(d) = q.pop_front() {
        if marked.insert(d)
            && let Some(parents) = rev.get(d)
        {
            for &p in parents {
                q.push_back(p);
            }
        }
    }

    // Return owned set
    marked.into_iter().cloned().collect()
}

/// Public API: Generate tree with caching
/// Uses the provided cache to avoid rebuilding identical trees
pub fn gen_drv_tree_leaves_from_state<'a>(
    cache: &'a mut TreeCache,
    state: &JobsStateInner,
    prune_mode: PruneType,
) -> &'a [TreeItem<'static, String>] {
    cache.get_or_build(state, prune_mode)
}

pub fn expand_all(
    tree_state: &mut TreeState<String>,
    roots: &[TreeItem<String>],
) {
    let mut q: VecDeque<(&TreeItem<String>, Vec<String>)> = VecDeque::new();

    // seed queue with each root and its 1-step path
    for root in roots {
        q.push_back((root, vec![root.identifier().clone()]));
    }

    while let Some((node, path)) = q.pop_front() {
        // IMPORTANT: pass the full path (root → … → this node)
        tree_state.open(path.clone());

        // enqueue children with extended paths
        for child in node.children() {
            let mut child_path = path.clone();
            child_path.push(child.identifier().clone());
            q.push_back((child, child_path));
        }
    }
}
