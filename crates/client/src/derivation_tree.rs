use std::{
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
    ops::Deref,
};

use either::Either::{Left, Right};
use serde::Deserialize;
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

// TODO rename to DrvTreeNode
#[derive(Clone, Debug, Default)]
pub struct DrvNode {
    pub root: Drv,
    pub deps: BTreeSet<Drv>,
    //pub outputs: HashSet<StoreOutput>,
}

#[derive(Clone, Debug, Default)]
pub struct ConcreteTree {
    pub node: Drv,
    pub children: BTreeSet<ConcreteTree>,
}

#[derive(Clone, Debug, Default)]
pub struct DrvRelations {
    // roots with info to form a tree
    pub nodes: BTreeMap<Drv, DrvNode>,
    // the "start" that we begin walking from
    // each time we insert a node, we check (recursively) for dependencies
    pub tree_roots: BTreeSet<Drv>,
}

impl DrvRelations {
    // only updates the nodes
    pub async fn insert(&mut self, drv: Drv) {
        if let Some(map) = drv.query_nix_about_drv().await {
            for (drv, derivation) in map.into_iter() {
                let node = DrvNode {
                    root: drv.clone(),
                    deps: derivation
                        .input_drvs
                        .into_keys()
                        .collect::<BTreeSet<_>>(),
                };
                self.nodes.insert(drv, node);
            }
        } else {
            error!("COULD NOT FIND DRV {} IN NIX STORE??", drv);
        }
    }

    // input drv: -> [[root1, d1, d2, ...], [root2, d1, d2, ...]]
    pub fn find_drv(&self, drv: Drv) -> Option<Vec<Vec<Drv>>> {
        unimplemented!()
    }

    fn handle_store_output(&self, so: StoreOutput) {
        unimplemented!()
    }

    // TODO benchmark perf vs why-depends, cuz this might be lowkey slower
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

    // right now does not care for efficiency. That comes later
    // only updates roots
    pub fn insert_node(&mut self, node: DrvNode) {
        let mut is_root = true;

        // node already is a root
        if self.tree_roots.contains(&node.root) {
            return;
        }

        // iterate through tree_roots, recursively searching for a dependency
        for a_root in &self.tree_roots {
            // check if in tree (recursive)
            if self.is_child_of(a_root, &node.root) {
                is_root = false;
            }
        }

        if is_root {
            // second: remove any children of node
            let mut new_nodes: BTreeSet<_> = self
                .tree_roots
                .clone()
                .into_iter()
                .filter(|r| !self.is_child_of(&node.root, r))
                .collect();

            // insert it as a root
            new_nodes.insert(node.root);
            self.tree_roots = new_nodes;
        }
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

fn drv_tree_of_derivation(name: String, value: Derivation) -> Option<DrvNode> {
    if let Left(drv) = parse_store_path(&name) {
        let deps = value.input_drvs.into_keys().collect::<BTreeSet<_>>();
        Some(DrvNode { root: drv, deps })
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
struct InputDrv {
    #[serde(default)]
    outputs: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Output {
    #[serde(default)]
    path: String,
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
