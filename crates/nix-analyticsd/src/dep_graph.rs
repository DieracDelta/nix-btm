//! Dependency graph manager for the daemon.
//!
//! Reads .drv files from /nix/store/ to discover dependency relationships,
//! tracks build status, and produces snapshots for the TUI.

use std::collections::{HashMap, HashSet};

use nix_analytics_common::dep_graph::{DepGraph, DepNode, DrvStatus, drv_name_from_path};

use crate::drv_parser;

/// Manages dependency graphs for all active nix command invocations.
pub struct DepGraphManager {
    /// Dependency graphs keyed by command_line.
    graphs: HashMap<String, DepGraph>,
    /// Maps activity_id -> (command_line, drv_path) for status updates.
    activity_to_drv: HashMap<u64, (String, String)>,
    /// drv_paths that have already been resolved (parsed from .drv files).
    resolved: HashSet<String>,
}

impl Default for DepGraphManager {
    fn default() -> Self {
        Self::new()
    }
}

impl DepGraphManager {
    pub fn new() -> Self {
        Self {
            graphs: HashMap::new(),
            activity_to_drv: HashMap::new(),
            resolved: HashSet::new(),
        }
    }

    /// Resolve a .drv file and recursively discover its dependencies.
    /// Creates the graph for `command_line` if it doesn't exist.
    /// Returns the list of newly discovered drv_paths that need resolving.
    pub fn resolve_drv(&mut self, drv_path: &str, command_line: &str) -> Vec<String> {
        if self.resolved.contains(drv_path) {
            // Already resolved, but ensure it's in this command's graph
            self.ensure_node_in_graph(drv_path, command_line);
            return Vec::new();
        }

        let graph = self
            .graphs
            .entry(command_line.to_string())
            .or_insert_with(|| DepGraph {
                command_line: Some(command_line.to_string()),
                ..Default::default()
            });

        // Read and parse the .drv file to discover dependencies.
        let input_drvs = match std::fs::read_to_string(drv_path) {
            Ok(content) => match drv_parser::parse_input_drvs(&content) {
                Ok(drvs) => drvs,
                Err(e) => {
                    tracing::warn!("failed to parse {drv_path}: {e}");
                    Vec::new()
                }
            },
            Err(e) => {
                tracing::debug!("cannot read {drv_path}: {e}");
                Vec::new()
            }
        };

        // All nodes start as Pending; the plugin will send DrvCached events
        // for derivations that were already in the store.
        let status = DrvStatus::Pending;

        let node = DepNode {
            drv_path: drv_path.to_string(),
            name: drv_name_from_path(drv_path),
            input_drvs: input_drvs.clone(),
            status,
            activity_id: None,
            started_at_us: None,
        };

        graph.nodes.insert(drv_path.to_string(), node);
        self.resolved.insert(drv_path.to_string());

        // Collect unresolved deps to return for recursive resolution
        let mut to_resolve = Vec::new();
        for dep in &input_drvs {
            if !self.resolved.contains(dep) {
                to_resolve.push(dep.clone());
            } else {
                // Already resolved, just make sure the node is in this graph
                self.ensure_node_in_graph(dep, command_line);
            }
        }

        to_resolve
    }

    /// Recursively resolve a drv and all its dependencies.
    pub fn resolve_drv_recursive(&mut self, drv_path: &str, command_line: &str) {
        let mut queue = vec![drv_path.to_string()];
        while let Some(path) = queue.pop() {
            let new_deps = self.resolve_drv(&path, command_line);
            queue.extend(new_deps);
        }
        self.recompute_roots(command_line);
    }

    /// Mark a derivation as currently building.
    pub fn set_building(&mut self, activity_id: u64, drv_path: &str, command_line: &str, started_at_us: u64) {
        self.activity_to_drv
            .insert(activity_id, (command_line.to_string(), drv_path.to_string()));

        if let Some(graph) = self.graphs.get_mut(command_line) {
            if let Some(node) = graph.nodes.get_mut(drv_path) {
                node.status = DrvStatus::Building { phase: None };
                node.activity_id = Some(activity_id);
                node.started_at_us = Some(started_at_us);
            }
        }
    }

    /// Mark a derivation as currently being substituted.
    pub fn set_substituting(&mut self, activity_id: u64, drv_path: &str, command_line: &str, started_at_us: u64) {
        self.activity_to_drv
            .insert(activity_id, (command_line.to_string(), drv_path.to_string()));

        if let Some(graph) = self.graphs.get_mut(command_line) {
            if let Some(node) = graph.nodes.get_mut(drv_path) {
                node.status = DrvStatus::Substituting {
                    done: None,
                    expected: None,
                };
                node.activity_id = Some(activity_id);
                node.started_at_us = Some(started_at_us);
            }
        }
    }

    /// Update the build phase for an activity.
    pub fn update_phase(&mut self, activity_id: u64, phase: &str) {
        if let Some((cmd, drv_path)) = self.activity_to_drv.get(&activity_id) {
            let cmd = cmd.clone();
            let drv_path = drv_path.clone();
            if let Some(graph) = self.graphs.get_mut(&cmd) {
                if let Some(node) = graph.nodes.get_mut(&drv_path) {
                    if let DrvStatus::Building { phase: ref mut p } = node.status {
                        *p = Some(phase.to_string());
                    }
                }
            }
        }
    }

    /// Update substitution progress for an activity.
    pub fn update_progress(&mut self, activity_id: u64, done: u64, expected: u64) {
        if let Some((cmd, drv_path)) = self.activity_to_drv.get(&activity_id) {
            let cmd = cmd.clone();
            let drv_path = drv_path.clone();
            if let Some(graph) = self.graphs.get_mut(&cmd) {
                if let Some(node) = graph.nodes.get_mut(&drv_path) {
                    if let DrvStatus::Substituting {
                        done: ref mut d,
                        expected: ref mut e,
                    } = node.status
                    {
                        *d = Some(done);
                        *e = Some(expected);
                    }
                }
            }
        }
    }

    /// Mark an activity as done.
    pub fn mark_done(&mut self, activity_id: u64) {
        if let Some((cmd, drv_path)) = self.activity_to_drv.remove(&activity_id) {
            if let Some(graph) = self.graphs.get_mut(&cmd) {
                if let Some(node) = graph.nodes.get_mut(&drv_path) {
                    node.status = DrvStatus::Done;
                    node.activity_id = None;
                }
            }
        }
    }

    /// Mark a derivation as cached (already in the store).
    /// Called when the plugin reports DrvCached (a Realise that completed
    /// without spawning a Build/Substitute child).
    pub fn mark_cached(&mut self, drv_path: &str, command_line: &str) {
        if let Some(graph) = self.graphs.get_mut(command_line) {
            if let Some(node) = graph.nodes.get_mut(drv_path) {
                node.status = DrvStatus::Done;
            }
        }
    }

    /// Mark an activity as failed.
    pub fn mark_failed(&mut self, activity_id: u64) {
        if let Some((cmd, drv_path)) = self.activity_to_drv.remove(&activity_id) {
            if let Some(graph) = self.graphs.get_mut(&cmd) {
                if let Some(node) = graph.nodes.get_mut(&drv_path) {
                    node.status = DrvStatus::Failed;
                    node.activity_id = None;
                }
            }
        }
    }

    /// Get a snapshot of all dependency graphs for the TUI.
    pub fn get_graphs(&self) -> Vec<DepGraph> {
        self.graphs.values().cloned().collect()
    }

    /// Ensure a node exists in a specific command's graph (for cross-graph dedup).
    fn ensure_node_in_graph(&mut self, drv_path: &str, command_line: &str) {
        // First, check if we need to copy from another graph.
        let existing_node = self
            .graphs
            .iter()
            .filter(|(k, _)| k.as_str() != command_line)
            .find_map(|(_, g)| g.nodes.get(drv_path))
            .cloned();

        let graph = self
            .graphs
            .entry(command_line.to_string())
            .or_insert_with(|| DepGraph {
                command_line: Some(command_line.to_string()),
                ..Default::default()
            });

        if !graph.nodes.contains_key(drv_path) {
            if let Some(node) = existing_node {
                graph.nodes.insert(drv_path.to_string(), node);
            }
        }
    }

    /// Recompute root nodes for a graph.
    /// Roots are nodes whose drv_path is not in any other node's input_drvs within the graph.
    fn recompute_roots(&mut self, command_line: &str) {
        if let Some(graph) = self.graphs.get_mut(command_line) {
            let referenced: HashSet<&str> = graph
                .nodes
                .values()
                .flat_map(|n| n.input_drvs.iter().map(|s| s.as_str()))
                .collect();

            let mut roots: Vec<String> = graph
                .nodes
                .keys()
                .filter(|k| !referenced.contains(k.as_str()))
                .cloned()
                .collect();
            // Sort for stable display order across renders.
            roots.sort();
            graph.roots = roots;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_manager_is_empty() {
        let mgr = DepGraphManager::new();
        assert!(mgr.get_graphs().is_empty());
    }

    #[test]
    fn set_building_and_phase() {
        let mut mgr = DepGraphManager::new();
        let cmd = "nix build .#foo";
        let drv = "/nix/store/abc-foo.drv";

        // Manually add a node (normally resolve_drv would do this)
        let graph = mgr.graphs.entry(cmd.to_string()).or_insert_with(|| DepGraph {
            command_line: Some(cmd.to_string()),
            ..Default::default()
        });
        graph.nodes.insert(drv.to_string(), DepNode {
            drv_path: drv.to_string(),
            name: "foo".to_string(),
            input_drvs: Vec::new(),
            status: DrvStatus::Pending,
            activity_id: None,
            started_at_us: None,
        });

        mgr.set_building(1, drv, cmd, 5000);
        let graphs = mgr.get_graphs();
        let node = graphs[0].nodes.get(drv).unwrap();
        assert!(matches!(node.status, DrvStatus::Building { .. }));
        assert_eq!(node.activity_id, Some(1));
        assert_eq!(node.started_at_us, Some(5000));

        mgr.update_phase(1, "buildPhase");
        let graphs = mgr.get_graphs();
        let node = graphs[0].nodes.get(drv).unwrap();
        match &node.status {
            DrvStatus::Building { phase } => assert_eq!(phase.as_deref(), Some("buildPhase")),
            _ => panic!("expected Building"),
        }
    }

    #[test]
    fn set_substituting_and_progress() {
        let mut mgr = DepGraphManager::new();
        let cmd = "nix build .#foo";
        let drv = "/nix/store/abc-foo.drv";

        let graph = mgr.graphs.entry(cmd.to_string()).or_insert_with(|| DepGraph {
            command_line: Some(cmd.to_string()),
            ..Default::default()
        });
        graph.nodes.insert(drv.to_string(), DepNode {
            drv_path: drv.to_string(),
            name: "foo".to_string(),
            input_drvs: Vec::new(),
            status: DrvStatus::Pending,
            activity_id: None,
            started_at_us: None,
        });

        mgr.set_substituting(2, drv, cmd, 6000);
        mgr.update_progress(2, 12_000_000, 50_000_000);

        let graphs = mgr.get_graphs();
        let node = graphs[0].nodes.get(drv).unwrap();
        match &node.status {
            DrvStatus::Substituting { done, expected } => {
                assert_eq!(*done, Some(12_000_000));
                assert_eq!(*expected, Some(50_000_000));
            }
            _ => panic!("expected Substituting"),
        }
    }

    #[test]
    fn mark_done_and_failed() {
        let mut mgr = DepGraphManager::new();
        let cmd = "nix build .#foo";
        let drv1 = "/nix/store/abc-foo.drv";
        let drv2 = "/nix/store/xyz-bar.drv";

        let graph = mgr.graphs.entry(cmd.to_string()).or_insert_with(|| DepGraph {
            command_line: Some(cmd.to_string()),
            ..Default::default()
        });
        for drv in [drv1, drv2] {
            graph.nodes.insert(drv.to_string(), DepNode {
                drv_path: drv.to_string(),
                name: drv_name_from_path(drv),
                input_drvs: Vec::new(),
                status: DrvStatus::Pending,
                activity_id: None,
                started_at_us: None,
            });
        }

        mgr.set_building(1, drv1, cmd, 5000);
        mgr.set_building(2, drv2, cmd, 5000);

        mgr.mark_done(1);
        mgr.mark_failed(2);

        let graphs = mgr.get_graphs();
        let n1 = graphs[0].nodes.get(drv1).unwrap();
        let n2 = graphs[0].nodes.get(drv2).unwrap();
        assert!(matches!(n1.status, DrvStatus::Done));
        assert!(matches!(n2.status, DrvStatus::Failed));
        assert!(n1.activity_id.is_none());
        assert!(n2.activity_id.is_none());
    }

    #[test]
    fn root_computation() {
        let mut mgr = DepGraphManager::new();
        let cmd = "nix build .#foo";
        let root_drv = "/nix/store/abc-foo.drv";
        let dep_drv = "/nix/store/xyz-bar.drv";

        let graph = mgr.graphs.entry(cmd.to_string()).or_insert_with(|| DepGraph {
            command_line: Some(cmd.to_string()),
            ..Default::default()
        });
        graph.nodes.insert(root_drv.to_string(), DepNode {
            drv_path: root_drv.to_string(),
            name: "foo".to_string(),
            input_drvs: vec![dep_drv.to_string()],
            status: DrvStatus::Pending,
            activity_id: None,
            started_at_us: None,
        });
        graph.nodes.insert(dep_drv.to_string(), DepNode {
            drv_path: dep_drv.to_string(),
            name: "bar".to_string(),
            input_drvs: Vec::new(),
            status: DrvStatus::Pending,
            activity_id: None,
            started_at_us: None,
        });

        mgr.recompute_roots(cmd);
        let graphs = mgr.get_graphs();
        assert_eq!(graphs[0].roots, vec![root_drv.to_string()]);
    }
}
