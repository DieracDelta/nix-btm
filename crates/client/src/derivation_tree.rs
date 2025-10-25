use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    ops::Deref,
};

use serde::Deserialize;
use tokio::process::Command;

use crate::handle_internal_json::Drv;
// detect a derivation -> insert into tree
// insertion step:
//   - if no other derivations: root
//   - else:
//     - iterate through derivation, and all its dependencies recursively
//     - tracking if we've seen any of them before
//     - check if it's a dependency of another derivation
//

#[derive(Clone, Debug, Default)]
pub struct DrvTree {
    pub root: Drv,
    pub children: Vec<DrvTree>,
}

#[derive(Clone, Debug, Default)]
pub struct DrvRelations {
    roots: Vec<DrvTree>,
    drvs: HashSet<Drv>,
}

impl Drv {
    pub async fn query_nix_about_drv(drv: Drv) -> Option<Derivation> {
        let path = format!("/nix/store/{}-{}.drv", drv.hash, drv.name);

        let output = Command::new("nix")
            .arg("derivation")
            .arg("show")
            .arg(&path)
            .output()
            .await
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let parsed: BTreeMap<Drv, Derivation> =
            serde_json::from_slice(&output.stdout).ok()?;

        parsed.into_values().next()
    }
}

impl DrvRelations {
    pub fn insert(&mut self, drv: Drv) {
        unimplemented!()
    }

    // input drv: -> [[root1, d1, d2, ...], [root2, d1, d2, ...]]
    pub fn find_drv(&self, drv: Drv) -> Option<Vec<Vec<Drv>>> {
        unimplemented!()
    }
}

impl DrvTree {
    fn contains(&self, d: &Drv) -> bool {
        let mut q: VecDeque<&DrvTree> = VecDeque::new();
        // really should not need a seen sets since supposedly this is a dag
        // but we have it anyway just in case to prevent infinite loop
        let mut seen: HashSet<&Drv> = HashSet::new();
        q.push_back(self);

        while let Some(node) = q.pop_front() {
            if &node.root == d {
                return true;
            }
            if !seen.insert(&node.root) {
                continue;
            }
            for child in &node.children {
                q.push_back(child);
            }
        }
        false
    }
}

#[derive(Debug, Deserialize)]
pub struct Derivation {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub system: String,
    #[serde(rename = "inputDrvs", default)]
    pub input_drvs: BTreeMap<String, InputDrv>,
    #[serde(default)]
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

    #[test]
    fn parse_derivation_and_edges() {
        // Deserialize top-level: { "<drv-path>": Derivation }
        let map: std::collections::BTreeMap<String, Derivation> =
            serde_json::from_str(NIX_DERIVATION_SHOW_OUTPUT)
                .expect("valid JSON");

        // We expect exactly one entry (the python3-static drv)
        assert_eq!(map.len(), 1);

        let (drv_key, drv) = map.into_iter().next().unwrap();

        assert_eq!(
            drv_key,
            "/nix/store/5sx0rmikbskbqvas9r6dyfs7g3pqhlhy-python3-static-x86_64-unknown-linux-musl-3.13.7.drv"
        );

        // Basic fields
        assert_eq!(drv.name, "python3-static-x86_64-unknown-linux-musl-3.13.7");
        assert_eq!(drv.system, "x86_64-linux");

        // Outputs → paths
        let out = drv.outputs.get("out").expect("out output present");
        assert_eq!(
            out.path,
            "/nix/store/s56pjzqnz4z83fs8ynnxk5dnslwjqgqd-python3-static-x86_64-unknown-linux-musl-3.13.7"
        );
        let dbg = drv.outputs.get("debug").expect("debug output present");
        assert_eq!(
            dbg.path,
            "/nix/store/hf3km28n98pm491ib64aw33yz3ificb4-python3-static-x86_64-unknown-linux-musl-3.13.7-debug"
        );

        // Edges (inputDrvs) — check two specific deps exist
        let sqlite = "/nix/store/06l593164qs7mhhx4688pki2clrx04sn-sqlite-static-x86_64-unknown-linux-musl-3.50.4.drv";
        let bzip2  = "/nix/store/4si7y5c10vnizgnzjrvnzmkxnalczk44-bzip2-static-x86_64-unknown-linux-musl-1.0.8.drv";

        let sqlite_edge =
            drv.input_drvs.get(sqlite).expect("sqlite dep present");
        let bzip2_edge = drv.input_drvs.get(bzip2).expect("bzip2 dep present");

        // Each declares which outputs of the dependency are used
        assert_eq!(sqlite_edge.outputs, vec!["dev", "out"]);
        assert_eq!(bzip2_edge.outputs, vec!["dev", "out"]);

        // Optional: prove the “X depends on Y” relation by building explicit
        // edge JSON
        let edges: Vec<_> = drv
            .input_drvs
            .iter()
            .map(|(to, info)| json!({ "from": drv_key, "to": to, "outputs": info.outputs }))
            .collect();

        // It should contain at least the two edges we asserted above
        assert!(edges.iter().any(|e| e["to"] == sqlite));
        assert!(edges.iter().any(|e| e["to"] == bzip2));
    }
}
