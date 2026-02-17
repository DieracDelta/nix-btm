//! Shared types for the dependency graph feature.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Status of a derivation in the dependency graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DrvStatus {
    Pending,
    Building { phase: Option<String> },
    Substituting { done: Option<u64>, expected: Option<u64> },
    Done,
    Failed,
}

/// A node in the dependency graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepNode {
    /// Full store path of the .drv file.
    pub drv_path: String,
    /// Short display name extracted from the drv path.
    pub name: String,
    /// Direct dependency drv paths (from inputDrvs).
    pub input_drvs: Vec<String>,
    /// Current build/substitution status.
    pub status: DrvStatus,
    /// Activity ID if this derivation is currently active.
    pub activity_id: Option<u64>,
    /// When the build/substitution started (microseconds since epoch).
    pub started_at_us: Option<u64>,
}

/// A dependency graph for a single nix command invocation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DepGraph {
    /// All nodes keyed by drv_path.
    pub nodes: HashMap<String, DepNode>,
    /// Top-level drv_paths (no incoming edges within this graph).
    pub roots: Vec<String>,
    /// The nix command line that spawned this graph.
    pub command_line: Option<String>,
}

/// Extract a short display name from a derivation store path.
/// "/nix/store/abc123-firefox-128.0.drv" -> "firefox-128.0"
pub fn drv_name_from_path(drv_path: &str) -> String {
    drv_path
        .rsplit('/')
        .next()
        .and_then(|name| name.split_once('-').map(|(_, rest)| rest))
        .map(|s| s.trim_end_matches(".drv").to_string())
        .unwrap_or_else(|| drv_path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drv_name_from_path_normal() {
        assert_eq!(
            drv_name_from_path("/nix/store/abc123-firefox-128.0.drv"),
            "firefox-128.0"
        );
    }

    #[test]
    fn drv_name_from_path_simple() {
        assert_eq!(
            drv_name_from_path("/nix/store/xyz-hello-2.12.drv"),
            "hello-2.12"
        );
    }

    #[test]
    fn drv_name_from_path_no_drv_suffix() {
        assert_eq!(
            drv_name_from_path("/nix/store/abc123-foo-1.0"),
            "foo-1.0"
        );
    }

    #[test]
    fn drv_name_from_path_bare_string() {
        assert_eq!(drv_name_from_path("foo"), "foo");
    }

    #[test]
    fn dep_graph_serde_roundtrip() {
        let mut nodes = HashMap::new();
        nodes.insert(
            "/nix/store/abc-foo.drv".to_string(),
            DepNode {
                drv_path: "/nix/store/abc-foo.drv".to_string(),
                name: "foo".to_string(),
                input_drvs: vec!["/nix/store/xyz-bar.drv".to_string()],
                status: DrvStatus::Building {
                    phase: Some("buildPhase".to_string()),
                },
                activity_id: Some(42),
                started_at_us: Some(1000),
            },
        );
        let graph = DepGraph {
            nodes,
            roots: vec!["/nix/store/abc-foo.drv".to_string()],
            command_line: Some("nix build .#foo".to_string()),
        };
        let json = serde_json::to_string(&graph).unwrap();
        let parsed: DepGraph = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.roots.len(), 1);
        assert_eq!(parsed.nodes.len(), 1);
        assert!(parsed.command_line.is_some());
    }

    #[test]
    fn dep_graph_msgpack_roundtrip() {
        let graph = DepGraph {
            nodes: HashMap::new(),
            roots: Vec::new(),
            command_line: None,
        };
        let bytes = rmp_serde::to_vec_named(&graph).unwrap();
        let parsed: DepGraph = rmp_serde::from_slice(&bytes).unwrap();
        assert!(parsed.nodes.is_empty());
    }

    #[test]
    fn drv_status_variants_serde() {
        for status in [
            DrvStatus::Pending,
            DrvStatus::Building { phase: Some("configurePhase".to_string()) },
            DrvStatus::Substituting { done: Some(12), expected: Some(50) },
            DrvStatus::Done,
            DrvStatus::Failed,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let _parsed: DrvStatus = serde_json::from_str(&json).unwrap();
        }
    }
}
