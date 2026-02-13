//! Shared types for the analytics system state.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::event::ActivityType;

/// Where a build is running.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BuildMachine {
    Local,
    Remote { uri: String },
}

/// Progress counters for an activity.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Progress {
    pub done: u64,
    pub expected: u64,
    pub running: u64,
    pub failed: u64,
}

/// A currently active build/activity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Build {
    pub activity_id: u64,
    pub activity_type: ActivityType,
    pub drv_path: Option<String>,
    pub description: String,
    pub parent_id: Option<u64>,

    /// The nix command line that spawned this activity (top-level only).
    pub command_line: Option<String>,

    /// When this build started (microseconds since epoch).
    pub started_at_us: u64,

    /// Current build phase (unpackPhase, buildPhase, etc.).
    pub phase: Option<String>,

    /// Last N log lines.
    pub recent_log: VecDeque<String>,

    /// Client info.
    pub user: Option<String>,
    pub user_pid: Option<u32>,
    pub user_uid: Option<u32>,

    /// Cgroup path for this build (if use-cgroups is enabled).
    pub cgroup_path: Option<PathBuf>,

    /// CPU time from cgroup stats.
    pub cpu_user_us: u64,
    pub cpu_system_us: u64,

    /// Current memory usage in bytes (from cgroup).
    pub memory_current: Option<u64>,

    /// Progress counters.
    pub progress: Option<Progress>,

    /// Where this build is running.
    pub machine: BuildMachine,
}

/// A completed build.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletedBuild {
    pub build: Build,
    pub finished_at_us: u64,
    pub duration: Duration,
    pub success: bool,
    pub cpu_user_total_us: u64,
    pub cpu_system_total_us: u64,
}

/// A remote builder machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteMachine {
    pub store_uri: String,
    pub system_types: Vec<String>,
    pub max_jobs: u32,
    pub speed_factor: f32,
    pub supported_features: Vec<String>,
    pub mandatory_features: Vec<String>,

    /// Number of currently active slots (from lock files).
    pub active_slots: u32,

    /// Activity IDs of builds on this machine.
    pub active_build_ids: Vec<u64>,
}

/// Full snapshot of the analytics state, used for TUI queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsSnapshot {
    pub active_builds: HashMap<u64, Build>,
    pub recent_history: Vec<CompletedBuild>,
    pub machines: Vec<RemoteMachine>,
    /// Dependency graphs for each active nix command.
    #[serde(default)]
    pub dep_graphs: Vec<crate::dep_graph::DepGraph>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn sample_build() -> Build {
        Build {
            activity_id: 1,
            activity_type: crate::event::ActivityType::Build,
            drv_path: Some("/nix/store/abc-foo-1.0.drv".to_string()),
            description: "building foo-1.0".to_string(),
            parent_id: None,
            command_line: None,
            started_at_us: 1000,
            phase: Some("buildPhase".to_string()),
            recent_log: VecDeque::from(vec!["line1".to_string()]),
            user: None,
            user_pid: None,
            user_uid: None,
            cgroup_path: None,
            cpu_user_us: 500,
            cpu_system_us: 200,
            memory_current: Some(1024 * 1024),
            progress: Some(Progress {
                done: 3,
                expected: 10,
                running: 2,
                failed: 0,
            }),
            machine: BuildMachine::Local,
        }
    }

    #[test]
    fn build_json_roundtrip() {
        let build = sample_build();
        let json = serde_json::to_string(&build).unwrap();
        let parsed: Build = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.activity_id, 1);
        assert_eq!(parsed.drv_path.as_deref(), Some("/nix/store/abc-foo-1.0.drv"));
    }

    #[test]
    fn build_msgpack_roundtrip() {
        let build = sample_build();
        let bytes = rmp_serde::to_vec_named(&build).unwrap();
        let parsed: Build = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(parsed.activity_id, 1);
        assert_eq!(parsed.phase.as_deref(), Some("buildPhase"));
    }

    #[test]
    fn completed_build_json_roundtrip() {
        let completed = CompletedBuild {
            build: sample_build(),
            finished_at_us: 2000,
            duration: Duration::from_secs(1),
            success: true,
            cpu_user_total_us: 500,
            cpu_system_total_us: 200,
        };
        let json = serde_json::to_string(&completed).unwrap();
        let parsed: CompletedBuild = serde_json::from_str(&json).unwrap();
        assert!(parsed.success);
        assert_eq!(parsed.finished_at_us, 2000);
    }

    #[test]
    fn completed_build_msgpack_roundtrip() {
        let completed = CompletedBuild {
            build: sample_build(),
            finished_at_us: 2000,
            duration: Duration::from_secs(1),
            success: false,
            cpu_user_total_us: 500,
            cpu_system_total_us: 200,
        };
        let bytes = rmp_serde::to_vec_named(&completed).unwrap();
        let parsed: CompletedBuild = rmp_serde::from_slice(&bytes).unwrap();
        assert!(!parsed.success);
    }

    #[test]
    fn analytics_snapshot_json_roundtrip() {
        let mut active = HashMap::new();
        active.insert(1, sample_build());
        let snapshot = AnalyticsSnapshot {
            active_builds: active,
            recent_history: Vec::new(),
            machines: Vec::new(),
            dep_graphs: Vec::new(),
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        let parsed: AnalyticsSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.active_builds.len(), 1);
    }

    #[test]
    fn analytics_snapshot_msgpack_roundtrip() {
        let snapshot = AnalyticsSnapshot {
            active_builds: HashMap::new(),
            recent_history: Vec::new(),
            dep_graphs: Vec::new(),
            machines: vec![RemoteMachine {
                store_uri: "ssh-ng://builder".to_string(),
                system_types: vec!["x86_64-linux".to_string()],
                max_jobs: 4,
                speed_factor: 2.0,
                supported_features: vec!["big-parallel".to_string()],
                mandatory_features: Vec::new(),
                active_slots: 1,
                active_build_ids: vec![42],
            }],
        };
        let bytes = rmp_serde::to_vec_named(&snapshot).unwrap();
        let parsed: AnalyticsSnapshot = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(parsed.machines.len(), 1);
        assert_eq!(parsed.machines[0].store_uri, "ssh-ng://builder");
    }

    #[test]
    fn build_machine_local_serde() {
        let machine = BuildMachine::Local;
        let json = serde_json::to_string(&machine).unwrap();
        let parsed: BuildMachine = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, BuildMachine::Local));
    }

    #[test]
    fn build_machine_remote_serde() {
        let machine = BuildMachine::Remote {
            uri: "ssh-ng://builder".to_string(),
        };
        let json = serde_json::to_string(&machine).unwrap();
        let parsed: BuildMachine = serde_json::from_str(&json).unwrap();
        match parsed {
            BuildMachine::Remote { uri } => assert_eq!(uri, "ssh-ng://builder"),
            _ => panic!("expected Remote"),
        }
    }
}
