use std::collections::{HashMap, HashSet, VecDeque};

use tracing::{error, info};
use tui_tree_widget::{TreeItem, TreeState};

use crate::handle_internal_json::{Drv, JobStatus, JobsStateInner};

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
) {
    // reshuffle if mismatched args (shouldn't be possible)
    let active = if prune == PruneType::None {
        None
    } else {
        active_closure
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
                    if state.get_status(child).is_active() {
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
                            if state.get_status(child).is_active() {
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
                            if state.get_status(&vis).is_active() {
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
                        state.make_tree_description(&vis),
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
pub fn gen_drv_tree_leaves_from_state(
    state: &JobsStateInner,
    do_prune: PruneType,
) -> Vec<TreeItem<'_, String>> {
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

    // Build mapping from drv to target using the new targets HashMap
    let mut drv_to_target_map: HashMap<&Drv, String> = HashMap::new();
    for target in state.targets.values() {
        drv_to_target_map.insert(&target.root_drv, target.reference.clone());
        error!("Target: '{}' with root drv: {}", target.reference, target.root_drv.name);
    }
    error!("Total targets: {}, Total dep_tree roots: {}", state.targets.len(), state.dep_tree.tree_roots.len());

    // Group roots by their target (flake reference)
    // Roots with a target get wrapped in a parent node showing the target
    // Roots without a target are shown directly
    let mut target_to_roots: std::collections::HashMap<String, Vec<&Drv>> =
        std::collections::HashMap::new();
    let mut orphan_roots: Vec<&Drv> = vec![];

    for a_root in &state.dep_tree.tree_roots {
        // For Normal pruning, skip roots that have no active descendants
        if let Some(ref ac) = active {
            if !ac.contains(a_root) {
                error!("  SKIPPING root {} - not in closure", a_root.name);
                continue;
            }
            error!("  KEEPING root {} - in closure", a_root.name);
        }

        if let Some(target) = drv_to_target_map.get(a_root) {
            target_to_roots
                .entry(target.clone())
                .or_default()
                .push(a_root);
        } else {
            orphan_roots.push(a_root);
        }
    }

    // Create tree items for roots with targets (target as parent, drv as child)
    for (target, drv_roots) in &target_to_roots {
        let mut children = vec![];

        for a_root in drv_roots {
            error!("building tree node for {a_root}");
            let mut drv_node = TreeItem::new(
                ((*a_root).clone()).to_string(),
                state.make_tree_description(a_root),
                vec![],
            )
            .unwrap();
            explore_root(
                &mut drv_node,
                state,
                a_root,
                do_prune,
                active.as_ref(),
            );

            // For Normal pruning, only include if root has children or is
            // itself active or completed
            let status = state.get_status(a_root);
            if do_prune == PruneType::Normal
                && drv_node.children().is_empty()
                && !status.is_active()
                && !status.is_completed()
                && !matches!(status, JobStatus::AlreadyBuilt)
            {
                error!("FILTERING OUT root {} - no children, not active, not completed", a_root.name);
                continue;
            }

            children.push(drv_node);
        }

        // Only add target node if it has children
        if !children.is_empty() {
            // Get the status from the first (usually only) drv root for this
            // target
            let drv_status = if let Some(first_drv) = drv_roots.first() {
                state.get_status(first_drv)
            } else {
                JobStatus::NotEnoughInfo
            };
            let target_display = format!("{} - {}", target, drv_status);

            let target_node =
                TreeItem::new(target.clone(), target_display, children)
                    .unwrap();
            roots.push(target_node);
        }
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
        explore_root(&mut new_root, state, a_root, do_prune, active.as_ref());

        // For Normal pruning, only include if root has children or is itself
        // active or completed
        let status = state.get_status(a_root);
        if do_prune == PruneType::Normal
            && new_root.children().is_empty()
            && !status.is_active()
            && !status.is_completed()
            && !matches!(status, JobStatus::AlreadyBuilt)
        {
            error!("FILTERING OUT orphan root {} - no children, not active, not completed", a_root.name);
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
