use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    sync::LazyLock,
    time::Instant,
};

use either::Either::{Left, Right};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::error;

use crate::handle_internal_json::{Drv, StoreOutput, parse_store_path};
// detect a derivation -> insert into tree
// insertion step:
//   - if no other derivations: root
//   - else:
//     - iterate through derivation, and all its dependencies recursively
//     - tracking if we've seen any of them before
//     - check if it's a dependency of another derivation
//

pub static START_INSTANT: LazyLock<Instant> = LazyLock::new(Instant::now);

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
)]
pub struct DrvNode {
    pub root: Drv,
    pub deps: BTreeSet<Drv>,
    /// Which output names of this drv are required by dependents
    /// e.g., {"out", "dev"} if something needs both outputs
    #[serde(default)]
    pub required_outputs: BTreeSet<String>,
    /// The actual store paths for the required outputs
    /// These can be checked directly to see if outputs exist
    #[serde(default)]
    pub required_output_paths: BTreeSet<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ConcreteTree {
    pub node: Drv,
    pub children: BTreeSet<ConcreteTree>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DrvRelations {
    // roots with info to form a tree
    pub nodes: BTreeMap<Drv, DrvNode>,
    // the "start" that we begin walking from
    // each time we insert a node, we check (recursively) for dependencies
    pub tree_roots: BTreeSet<Drv>,
}

impl DrvRelations {
    // Updates nodes and marks the queried drv as a potential root
    pub async fn insert(&mut self, drv: Drv) {
        if let Some(map) = drv.query_nix_about_drv().await {
            // First pass: collect which outputs each drv needs
            // Map from drv -> set of required output names
            let mut required_outputs_map: BTreeMap<Drv, BTreeSet<String>> = BTreeMap::new();

            for (_d, derivation) in &map {
                for (dep_drv, input_drv) in &derivation.input_drvs {
                    required_outputs_map
                        .entry(dep_drv.clone())
                        .or_default()
                        .extend(input_drv.outputs.iter().cloned());
                }
            }

            // Add all nodes to the graph
            for (d, derivation) in map.into_iter() {
                // Get required outputs for this drv, default to "out" for root
                let required_outputs = required_outputs_map
                    .get(&d)
                    .cloned()
                    .unwrap_or_else(|| {
                        // For root drv, default to "out" output
                        let mut set = BTreeSet::new();
                        set.insert("out".to_string());
                        set
                    });

                // Get the actual output paths for the required outputs
                // Note: nix derivation show returns paths without /nix/store/ prefix
                let mut required_output_paths: BTreeSet<String> = required_outputs
                    .iter()
                    .filter_map(|output_name| {
                        derivation.outputs.get(output_name).map(|o| {
                            if o.path.starts_with("/nix/store/") {
                                o.path.clone()
                            } else if !o.path.is_empty() {
                                format!("/nix/store/{}", o.path)
                            } else {
                                String::new()
                            }
                        })
                    })
                    .filter(|p| !p.is_empty())
                    .collect();

                // For FODs (fixed-output derivations), paths are empty in nix derivation show
                // Query nix-store directly to get output paths
                if required_output_paths.is_empty() && !required_outputs.is_empty() {
                    let drv_path = format!("/nix/store/{}-{}.drv", d.hash, d.name);
                    if let Ok(output) = Command::new("nix-store")
                        .arg("-q")
                        .arg("--outputs")
                        .arg(&drv_path)
                        .output()
                        .await
                    {
                        if output.status.success() {
                            let stdout = String::from_utf8_lossy(&output.stdout);
                            for line in stdout.lines() {
                                let path = line.trim();
                                if !path.is_empty() {
                                    required_output_paths.insert(path.to_string());
                                }
                            }
                        }
                    }
                }

                let node = DrvNode {
                    root: d.clone(),
                    deps: derivation
                        .input_drvs
                        .into_keys()
                        .collect::<BTreeSet<_>>(),
                    required_outputs,
                    required_output_paths,
                };
                self.nodes.insert(d, node);
            }
            // Recalculate all roots based on the updated graph
            self.recalculate_roots();
        } else {
            error!("COULD NOT FIND DRV {} IN NIX STORE??", drv);
        }
    }

    // TODO benchmark perf vs why-depends, cuz this might be lowkey slower
    // ofc profile first
    fn is_child_of(&self, parent_drv: &Drv, child_drv: &Drv) -> bool {
        // unwrap fine b/c impossible for the node to be in the roots but not in
        // nodes
        let parent_node = self.nodes.get(parent_drv).unwrap();
        let mut stack = parent_node.deps.iter().collect::<Vec<_>>();
        let mut visited: HashSet<&Drv> = HashSet::new();

        while let Some(node) = stack.pop() {
            if node == child_drv {
                return true;
            }
            visited.insert(node);
            if let Some(children) = self.nodes.get(node).map(|x| &x.deps) {
                let f_children: Vec<&Drv> =
                    children.iter().filter(|x| !visited.contains(x)).collect();
                stack.extend(f_children);
            }
        }
        false
    }

    // Updates roots when a new node is added
    // A node is a root if it has no parents (nothing depends on it)
    pub fn insert_node(&mut self, node: DrvNode) {
        let node_name = node.root.name.clone();

        // Remove any of node's dependencies from roots
        // (they now have a parent - this node)
        self.tree_roots.retain(|r| !node.deps.contains(r));

        // Check if this node has any parents (is a dependency of any existing node)
        let has_parent = self.nodes.iter().any(|(drv, n)| {
            drv != &node.root && n.deps.contains(&node.root)
        });

        // If no parent found, this is a root
        if !has_parent && !self.tree_roots.contains(&node.root) {
            self.tree_roots.insert(node.root);
            tracing::debug!("insert_node: added {} as root", node_name);
        } else {
            tracing::debug!("insert_node: {} NOT added as root (has_parent={})", node_name, has_parent);
        }
    }

    /// Recalculate all roots from scratch based on current graph
    /// A root is any node that no other node depends on
    pub fn recalculate_roots(&mut self) {
        // Collect all nodes that are dependencies of something
        let mut has_parent: BTreeSet<Drv> = BTreeSet::new();
        for (_drv, node) in &self.nodes {
            for dep in &node.deps {
                has_parent.insert(dep.clone());
            }
        }

        // Roots are nodes that have no parent
        self.tree_roots = self.nodes.keys()
            .filter(|d| !has_parent.contains(*d))
            .cloned()
            .collect();

        tracing::debug!("recalculate_roots: {} roots from {} nodes: {:?}",
            self.tree_roots.len(), self.nodes.len(),
            self.tree_roots.iter().map(|r| &r.name).collect::<Vec<_>>());

        // Log nodes that have parents for debugging
        let with_parents: Vec<_> = self.nodes.keys()
            .filter(|d| has_parent.contains(*d))
            .map(|d| &d.name)
            .collect();
        if !with_parents.is_empty() {
            tracing::debug!("nodes with parents (not roots): {:?}", with_parents);
        }
    }

    // Check if child_drv is a dependency of parent_drv (directly or recursively)
    fn is_child_of_direct_or_recursive(
        &self,
        parent_drv: &Drv,
        child_drv: &Drv,
    ) -> bool {
        if let Some(parent_node) = self.nodes.get(parent_drv) {
            if parent_node.deps.contains(child_drv) {
                return true;
            }
            for dep in &parent_node.deps {
                if self.is_child_of_direct_or_recursive(dep, child_drv) {
                    return true;
                }
            }
        }
        false
    }
}

impl StoreOutput {
    // gets the drv this output path is associated with
    pub async fn get_drv(&self) -> Option<Drv> {
        let output = Command::new("nix-store")
            .arg("-qd")
            .arg(format!("/nix/store/{}-{}", self.hash, self.name))
            .output()
            .await
            .ok()?;

        if !output.status.success() {
            None
        } else {
            let tmp = String::from_utf8_lossy(&output.stdout);
            let stdout = tmp.trim();
            match parse_store_path(stdout) {
                Left(drv) => Some(drv),
                Right(_) => None,
            }
        }
    }
}

impl Drv {
    // call with the recursive flag. Do the necessary insertions.
    pub async fn query_nix_about_drv(
        &self,
    ) -> Option<BTreeMap<Drv, Derivation>> {
        let path = format!("/nix/store/{}-{}.drv", self.hash, self.name);
        let output = Command::new("nix")
            .arg("derivation")
            .arg("show")
            .arg("--recursive")
            .arg(&path)
            .output()
            .await
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let parsed: BTreeMap<Drv, Derivation> =
            serde_json::from_slice(&output.stdout).ok()?;
        Some(parsed)
    }
}

pub fn drv_tree_of_derivation(
    name: String,
    value: Derivation,
) -> Option<DrvNode> {
    if let Left(drv) = parse_store_path(&name) {
        let deps = value.input_drvs.into_keys().collect::<BTreeSet<_>>();
        Some(DrvNode { root: drv, deps, required_outputs: BTreeSet::new(), required_output_paths: BTreeSet::new() })
    } else {
        error!("{name} wasn't a drv");
        None
    }
}

#[derive(Debug, Deserialize)]
pub struct Derivation {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub system: String,
    #[serde(rename = "inputDrvs", default)]
    pub input_drvs: BTreeMap<Drv, InputDrv>,
    #[serde(default)] // probably don't care about this just yet
    pub outputs: BTreeMap<String, Output>,
}

#[derive(Debug, Deserialize)]
pub struct InputDrv {
    #[allow(dead_code)]
    #[serde(default)]
    outputs: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Output {
    #[serde(default)]
    pub path: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const NIX_DERIVATION_SHOW_OUTPUT: &str = r#"
    {
      "/nix/store/5sx0rmikbskbqvas9r6dyfs7g3pqhlhy-python3-static-x86_64-unknown-linux-musl-3.13.7.drv": {
        "name": "python3-static-x86_64-unknown-linux-musl-3.13.7",
        "system": "x86_64-linux",
        "inputDrvs": {
          "/nix/store/06l593164qs7mhhx4688pki2clrx04sn-sqlite-static-x86_64-unknown-linux-musl-3.50.4.drv": {
            "dynamicOutputs": {},
            "outputs": ["dev", "out"]
          },
          "/nix/store/4si7y5c10vnizgnzjrvnzmkxnalczk44-bzip2-static-x86_64-unknown-linux-musl-1.0.8.drv": {
            "dynamicOutputs": {},
            "outputs": ["dev", "out"]
          }
        },
        "outputs": {
          "out":   { "path": "/nix/store/s56pjzqnz4z83fs8ynnxk5dnslwjqgqd-python3-static-x86_64-unknown-linux-musl-3.13.7" },
          "debug": { "path": "/nix/store/hf3km28n98pm491ib64aw33yz3ificb4-python3-static-x86_64-unknown-linux-musl-3.13.7-debug" }
        }
      }
    }
    "#;

    // Helper to build a Drv from a store path string (panics on non-.drv)
    fn drv(path: &str) -> Drv {
        match parse_store_path(path) {
            either::Either::Left(d) => d,
            _ => panic!("expected .drv path: {path}"),
        }
    }

    #[test]
    fn parse_derivation_and_edges() {
        // ✅ Deserialize top-level as { Drv: Derivation }
        let map: std::collections::BTreeMap<Drv, Derivation> =
            serde_json::from_str(NIX_DERIVATION_SHOW_OUTPUT)
                .expect("valid JSON");

        assert_eq!(map.len(), 1);

        let (top_drv, drv_val) = map.into_iter().next().unwrap();

        // Top-level key equals expected Drv (no .drv in name)
        assert_eq!(
            top_drv,
            drv("/nix/store/5sx0rmikbskbqvas9r6dyfs7g3pqhlhy-python3-static-x86_64-unknown-linux-musl-3.13.7.drv")
        );

        // Basic fields
        assert_eq!(
            drv_val.name,
            "python3-static-x86_64-unknown-linux-musl-3.13.7"
        );
        assert_eq!(drv_val.system, "x86_64-linux");

        // Outputs → paths
        let out = drv_val.outputs.get("out").expect("out output present");
        assert_eq!(
            out.path,
            "/nix/store/s56pjzqnz4z83fs8ynnxk5dnslwjqgqd-python3-static-x86_64-unknown-linux-musl-3.13.7"
        );
        let dbg = drv_val.outputs.get("debug").expect("debug output present");
        assert_eq!(
            dbg.path,
            "/nix/store/hf3km28n98pm491ib64aw33yz3ificb4-python3-static-x86_64-unknown-linux-musl-3.13.7-debug"
        );

        // Edges (inputDrvs) — look up with Drv keys
        let sqlite = drv("/nix/store/06l593164qs7mhhx4688pki2clrx04sn-sqlite-static-x86_64-unknown-linux-musl-3.50.4.drv");
        let bzip2  = drv("/nix/store/4si7y5c10vnizgnzjrvnzmkxnalczk44-bzip2-static-x86_64-unknown-linux-musl-1.0.8.drv");

        let sqlite_edge =
            drv_val.input_drvs.get(&sqlite).expect("sqlite dep present");
        let bzip2_edge =
            drv_val.input_drvs.get(&bzip2).expect("bzip2 dep present");

        assert_eq!(sqlite_edge.outputs, vec!["dev", "out"]);
        assert_eq!(bzip2_edge.outputs, vec!["dev", "out"]);

        // Optional: build edges JSON (note: from/to are strings here)
        let from = format!("/nix/store/{}-{}.drv", top_drv.hash, top_drv.name);
        let edges: Vec<_> = drv_val
            .input_drvs
            .iter()
            .map(|(to, info)| {
                let to = format!("/nix/store/{}-{}.drv", to.hash, to.name);
                json!({ "from": from, "to": to, "outputs": info.outputs })
            })
            .collect();

        assert!(edges.iter().any(|e| e["to"] == "/nix/store/06l593164qs7mhhx4688pki2clrx04sn-sqlite-static-x86_64-unknown-linux-musl-3.50.4.drv"));
    }
}
