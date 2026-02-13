//! Events emitted by the nix-daemon plugin.
//!
//! These are the structured events that the C++ plugin serializes and sends
//! over the analytics Unix socket. The analytics daemon deserializes them
//! to build its live state model.

use serde::{Deserialize, Serialize};

/// Nix activity types, mirroring the ActivityType enum in logging.hh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u16)]
pub enum ActivityType {
    Unknown = 0,
    CopyPath = 100,
    FileTransfer = 101,
    Realise = 102,
    CopyPaths = 103,
    Builds = 104,
    Build = 105,
    OptimiseStore = 106,
    VerifyPaths = 107,
    Substitute = 108,
    QueryPathInfo = 109,
    PostBuildHook = 110,
    BuildWaiting = 111,
    FetchTree = 112,
}

impl std::fmt::Display for ActivityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Build => "build",
            Self::CopyPath => "copy",
            Self::CopyPaths => "copy paths",
            Self::FileTransfer => "download",
            Self::FetchTree => "fetch",
            Self::Substitute => "substitute",
            Self::Realise => "realise",
            Self::Builds => "builds",
            Self::BuildWaiting => "waiting",
            Self::OptimiseStore => "optimise",
            Self::VerifyPaths => "verify",
            Self::QueryPathInfo => "query",
            Self::PostBuildHook => "post-hook",
            Self::Unknown => "-",
        };
        f.write_str(label)
    }
}

impl From<u16> for ActivityType {
    fn from(v: u16) -> Self {
        match v {
            100 => Self::CopyPath,
            101 => Self::FileTransfer,
            102 => Self::Realise,
            103 => Self::CopyPaths,
            104 => Self::Builds,
            105 => Self::Build,
            106 => Self::OptimiseStore,
            107 => Self::VerifyPaths,
            108 => Self::Substitute,
            109 => Self::QueryPathInfo,
            110 => Self::PostBuildHook,
            111 => Self::BuildWaiting,
            112 => Self::FetchTree,
            _ => Self::Unknown,
        }
    }
}

/// Events emitted by the plugin's Logger wrapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AnalyticsEvent {
    /// An activity (build, substitution, copy, etc.) started.
    ActivityStarted {
        timestamp_us: u64,
        activity_id: u64,
        activity_type: u16,
        /// Human-readable description (e.g. "building foo-1.0").
        description: String,
        /// For builds: the derivation path.
        drv_path: Option<String>,
        /// Parent activity ID (for nesting).
        parent_id: u64,
        /// The nix command line (only set on top-level activities).
        #[serde(default)]
        command_line: Option<String>,
        /// PID of the nix daemon fork (for fallback when cgroups are unavailable).
        #[serde(default)]
        nix_pid: Option<u32>,
    },

    /// An activity stopped.
    ActivityStopped {
        timestamp_us: u64,
        activity_id: u64,
    },

    /// A build's phase changed (e.g. unpackPhase → buildPhase).
    PhaseChanged {
        timestamp_us: u64,
        activity_id: u64,
        phase: String,
    },

    /// A build log line was emitted.
    LogLine {
        timestamp_us: u64,
        activity_id: u64,
        text: String,
    },

    /// Progress update for an activity.
    Progress {
        timestamp_us: u64,
        activity_id: u64,
        done: u64,
        expected: u64,
        running: u64,
        failed: u64,
    },

    /// A post-build hook log line.
    PostBuildLogLine {
        timestamp_us: u64,
        activity_id: u64,
        text: String,
    },

    /// Build was dispatched to a remote machine.
    RemoteDispatch {
        timestamp_us: u64,
        activity_id: u64,
        machine_uri: String,
    },

    /// A derivation was already cached in the store (Realise completed
    /// without spawning a Build or Substitute child).
    DrvCached {
        timestamp_us: u64,
        activity_id: u64,
        drv_path: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_type_from_known_values() {
        assert_eq!(ActivityType::from(100), ActivityType::CopyPath);
        assert_eq!(ActivityType::from(101), ActivityType::FileTransfer);
        assert_eq!(ActivityType::from(102), ActivityType::Realise);
        assert_eq!(ActivityType::from(103), ActivityType::CopyPaths);
        assert_eq!(ActivityType::from(104), ActivityType::Builds);
        assert_eq!(ActivityType::from(105), ActivityType::Build);
        assert_eq!(ActivityType::from(106), ActivityType::OptimiseStore);
        assert_eq!(ActivityType::from(107), ActivityType::VerifyPaths);
        assert_eq!(ActivityType::from(108), ActivityType::Substitute);
        assert_eq!(ActivityType::from(109), ActivityType::QueryPathInfo);
        assert_eq!(ActivityType::from(110), ActivityType::PostBuildHook);
        assert_eq!(ActivityType::from(111), ActivityType::BuildWaiting);
        assert_eq!(ActivityType::from(112), ActivityType::FetchTree);
    }

    #[test]
    fn activity_type_from_unknown_values() {
        assert_eq!(ActivityType::from(0), ActivityType::Unknown);
        assert_eq!(ActivityType::from(99), ActivityType::Unknown);
        assert_eq!(ActivityType::from(113), ActivityType::Unknown);
        assert_eq!(ActivityType::from(u16::MAX), ActivityType::Unknown);
    }

    #[test]
    fn json_roundtrip_activity_started() {
        let event = AnalyticsEvent::ActivityStarted {
            timestamp_us: 1000,
            activity_id: 42,
            activity_type: 105,
            description: "building foo-1.0".to_string(),
            drv_path: Some("/nix/store/abc-foo-1.0.drv".to_string()),
            parent_id: 0,
            command_line: None,
            nix_pid: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: AnalyticsEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, AnalyticsEvent::ActivityStarted { activity_id: 42, .. }));
    }

    #[test]
    fn json_roundtrip_activity_stopped() {
        let event = AnalyticsEvent::ActivityStopped {
            timestamp_us: 2000,
            activity_id: 42,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: AnalyticsEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, AnalyticsEvent::ActivityStopped { activity_id: 42, .. }));
    }

    #[test]
    fn json_roundtrip_phase_changed() {
        let event = AnalyticsEvent::PhaseChanged {
            timestamp_us: 3000,
            activity_id: 42,
            phase: "buildPhase".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: AnalyticsEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, AnalyticsEvent::PhaseChanged { .. }));
    }

    #[test]
    fn json_roundtrip_log_line() {
        let event = AnalyticsEvent::LogLine {
            timestamp_us: 4000,
            activity_id: 42,
            text: "compiling main.c".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: AnalyticsEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, AnalyticsEvent::LogLine { .. }));
    }

    #[test]
    fn json_roundtrip_progress() {
        let event = AnalyticsEvent::Progress {
            timestamp_us: 5000,
            activity_id: 42,
            done: 3,
            expected: 10,
            running: 2,
            failed: 0,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: AnalyticsEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, AnalyticsEvent::Progress { done: 3, expected: 10, .. }));
    }

    #[test]
    fn json_roundtrip_post_build_log_line() {
        let event = AnalyticsEvent::PostBuildLogLine {
            timestamp_us: 6000,
            activity_id: 42,
            text: "signing path".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: AnalyticsEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, AnalyticsEvent::PostBuildLogLine { .. }));
    }

    #[test]
    fn json_roundtrip_remote_dispatch() {
        let event = AnalyticsEvent::RemoteDispatch {
            timestamp_us: 7000,
            activity_id: 42,
            machine_uri: "ssh-ng://builder".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: AnalyticsEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, AnalyticsEvent::RemoteDispatch { .. }));
    }

    /// Contract test: hardcoded JSON matching C++ plugin output deserializes correctly.
    #[test]
    fn plugin_json_contract_activity_started() {
        let json = r#"{"type":"ActivityStarted","timestamp_us":1234567890,"activity_id":1,"activity_type":105,"description":"building foo-1.0","drv_path":"/nix/store/abc-foo-1.0.drv","parent_id":0}"#;
        let event: AnalyticsEvent = serde_json::from_str(json).unwrap();
        match event {
            AnalyticsEvent::ActivityStarted {
                timestamp_us,
                activity_id,
                activity_type,
                description,
                drv_path,
                parent_id,
                command_line,
                nix_pid,
            } => {
                assert_eq!(timestamp_us, 1234567890);
                assert_eq!(activity_id, 1);
                assert_eq!(activity_type, 105);
                assert_eq!(description, "building foo-1.0");
                assert_eq!(drv_path.as_deref(), Some("/nix/store/abc-foo-1.0.drv"));
                assert_eq!(parent_id, 0);
                assert!(command_line.is_none());
                assert!(nix_pid.is_none());
            }
            _ => panic!("expected ActivityStarted"),
        }
    }

    /// drv_path: null → None
    #[test]
    fn plugin_json_contract_null_drv_path() {
        let json = r#"{"type":"ActivityStarted","timestamp_us":1000,"activity_id":2,"activity_type":100,"description":"copying path","drv_path":null,"parent_id":0}"#;
        let event: AnalyticsEvent = serde_json::from_str(json).unwrap();
        match event {
            AnalyticsEvent::ActivityStarted { drv_path, .. } => {
                assert!(drv_path.is_none());
            }
            _ => panic!("expected ActivityStarted"),
        }
    }

    #[test]
    fn plugin_json_contract_activity_stopped() {
        let json = r#"{"type":"ActivityStopped","timestamp_us":2000,"activity_id":1}"#;
        let event: AnalyticsEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, AnalyticsEvent::ActivityStopped { activity_id: 1, .. }));
    }

    #[test]
    fn plugin_json_contract_phase_changed() {
        let json = r#"{"type":"PhaseChanged","timestamp_us":3000,"activity_id":1,"phase":"buildPhase"}"#;
        let event: AnalyticsEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, AnalyticsEvent::PhaseChanged { .. }));
    }

    #[test]
    fn plugin_json_contract_progress() {
        let json = r#"{"type":"Progress","timestamp_us":4000,"activity_id":1,"done":5,"expected":10,"running":2,"failed":0}"#;
        let event: AnalyticsEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, AnalyticsEvent::Progress { done: 5, expected: 10, .. }));
    }

    #[test]
    fn json_roundtrip_drv_cached() {
        let event = AnalyticsEvent::DrvCached {
            timestamp_us: 8000,
            activity_id: 42,
            drv_path: "/nix/store/abc-foo.drv".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: AnalyticsEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, AnalyticsEvent::DrvCached { activity_id: 42, .. }));
    }

    #[test]
    fn plugin_json_contract_drv_cached() {
        let json = r#"{"type":"DrvCached","timestamp_us":5000,"activity_id":3,"drv_path":"/nix/store/abc-foo.drv"}"#;
        let event: AnalyticsEvent = serde_json::from_str(json).unwrap();
        match event {
            AnalyticsEvent::DrvCached { activity_id, drv_path, .. } => {
                assert_eq!(activity_id, 3);
                assert_eq!(drv_path, "/nix/store/abc-foo.drv");
            }
            _ => panic!("expected DrvCached"),
        }
    }
}
