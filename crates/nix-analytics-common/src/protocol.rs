//! Wire protocol between the TUI client and the analytics daemon.
//!
//! Communication happens over a Unix domain socket. Messages are
//! length-prefixed MessagePack:
//!
//!   [4 bytes: big-endian u32 length][MessagePack payload]

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::types::{AnalyticsSnapshot, Build, CompletedBuild, RemoteMachine};

/// Default path for the plugin → daemon event socket.
pub const DEFAULT_EVENT_SOCKET: &str = "/run/nix-analytics/events.sock";

/// Default path for the TUI → daemon control socket.
pub const DEFAULT_CONTROL_SOCKET: &str = "/run/nix-analytics/control.sock";

/// Resolve the socket directory.
///
/// Priority:
/// 1. `NIX_ANALYTICS_SOCKET_DIR` environment variable (explicit override)
/// 2. `$XDG_RUNTIME_DIR/nix-analytics` if it exists and has a socket (rootless nix)
/// 3. `/run/nix-analytics` (system default)
pub fn socket_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("NIX_ANALYTICS_SOCKET_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        let user_dir = PathBuf::from(xdg).join("nix-analytics");
        if user_dir.join("control.sock").exists() || user_dir.join("events.sock").exists() {
            return user_dir;
        }
    }
    PathBuf::from("/run/nix-analytics")
}

/// Resolve the event socket path (plugin → daemon).
pub fn event_socket_path() -> PathBuf {
    socket_dir().join("events.sock")
}

/// Resolve the control socket path (TUI/ctl → daemon).
pub fn control_socket_path() -> PathBuf {
    socket_dir().join("control.sock")
}

/// Maximum number of log lines to keep per build.
pub const MAX_LOG_LINES: usize = 500;

/// Maximum number of completed builds to keep in history.
pub const MAX_HISTORY: usize = 1000;

/// Action to perform on a build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BuildAction {
    Kill,     // cgroup.kill / SIGKILL
    Freeze,   // cgroup.freeze=1 / SIGSTOP
    Unfreeze, // cgroup.freeze=0 / SIGCONT
}

// -- Requests from TUI to daemon --

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd")]
pub enum Request {
    /// Get a full snapshot of current state.
    GetSnapshot,

    /// List all active builds.
    ListBuilds,

    /// Get details for a specific build.
    GetBuild { activity_id: u64 },

    /// Get recent log lines for a build.
    GetBuildLog { activity_id: u64, last_n: usize },

    /// List remote builder machines and their status.
    ListMachines,

    /// Get recent build history.
    GetHistory { last_n: usize },

    /// Control a running build (kill, freeze, or unfreeze).
    ControlBuild { activity_id: u64, action: BuildAction },

    /// Set the CPU scheduling weight (niceness) for a build's cgroup.
    /// Range: 1-10000, default 100.
    SetNice { activity_id: u64, weight: u32 },

    /// Set CPU limit for a build's cgroup.
    /// Expressed as a percentage of one core (e.g. 200 = 2 cores).
    SetCpuLimit { activity_id: u64, percent: u32 },

    /// Set memory limit for a build's cgroup, in bytes.
    SetMemoryLimit { activity_id: u64, bytes: u64 },
}

// -- Responses from daemon to TUI --

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum Response {
    Snapshot {
        snapshot: AnalyticsSnapshot,
    },
    Builds {
        builds: Vec<Build>,
    },
    Build {
        build: Option<Box<Build>>,
    },
    BuildLog {
        activity_id: u64,
        lines: Vec<String>,
    },
    Machines {
        machines: Vec<RemoteMachine>,
    },
    History {
        builds: Vec<CompletedBuild>,
    },
    Ok,
    Error {
        message: String,
    },
}

/// Encode a message as length-prefixed MessagePack bytes.
pub fn encode_message<T: Serialize>(msg: &T) -> Result<Vec<u8>, rmp_serde::encode::Error> {
    let payload = rmp_serde::to_vec_named(msg)?;
    let len = (payload.len() as u32).to_be_bytes();
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.extend_from_slice(&len);
    buf.extend_from_slice(&payload);
    Ok(buf)
}

/// Decode a MessagePack payload (without the length prefix).
pub fn decode_message<'a, T: Deserialize<'a>>(data: &'a [u8]) -> Result<T, rmp_serde::decode::Error> {
    rmp_serde::from_slice(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn roundtrip_request(req: &Request) {
        let encoded = encode_message(req).unwrap();
        // First 4 bytes are the length prefix.
        let len = u32::from_be_bytes(encoded[..4].try_into().unwrap()) as usize;
        assert_eq!(len, encoded.len() - 4);
        let decoded: Request = decode_message(&encoded[4..]).unwrap();
        // Verify the tag matches by re-encoding.
        let re_encoded = encode_message(&decoded).unwrap();
        assert_eq!(encoded, re_encoded);
    }

    fn roundtrip_response(resp: &Response) {
        let encoded = encode_message(resp).unwrap();
        let len = u32::from_be_bytes(encoded[..4].try_into().unwrap()) as usize;
        assert_eq!(len, encoded.len() - 4);
        let decoded: Response = decode_message(&encoded[4..]).unwrap();
        let re_encoded = encode_message(&decoded).unwrap();
        assert_eq!(encoded, re_encoded);
    }

    #[test]
    fn request_roundtrip_get_snapshot() {
        roundtrip_request(&Request::GetSnapshot);
    }

    #[test]
    fn request_roundtrip_list_builds() {
        roundtrip_request(&Request::ListBuilds);
    }

    #[test]
    fn request_roundtrip_get_build() {
        roundtrip_request(&Request::GetBuild { activity_id: 42 });
    }

    #[test]
    fn request_roundtrip_get_build_log() {
        roundtrip_request(&Request::GetBuildLog {
            activity_id: 42,
            last_n: 100,
        });
    }

    #[test]
    fn request_roundtrip_list_machines() {
        roundtrip_request(&Request::ListMachines);
    }

    #[test]
    fn request_roundtrip_get_history() {
        roundtrip_request(&Request::GetHistory { last_n: 50 });
    }

    #[test]
    fn request_roundtrip_control_build() {
        for action in [BuildAction::Kill, BuildAction::Freeze, BuildAction::Unfreeze] {
            roundtrip_request(&Request::ControlBuild {
                activity_id: 7,
                action,
            });
        }
    }

    #[test]
    fn request_roundtrip_set_nice() {
        roundtrip_request(&Request::SetNice {
            activity_id: 7,
            weight: 10,
        });
    }

    #[test]
    fn request_roundtrip_set_cpu_limit() {
        roundtrip_request(&Request::SetCpuLimit {
            activity_id: 7,
            percent: 200,
        });
    }

    #[test]
    fn request_roundtrip_set_memory_limit() {
        roundtrip_request(&Request::SetMemoryLimit {
            activity_id: 7,
            bytes: 4 * 1024 * 1024 * 1024,
        });
    }

    #[test]
    fn response_roundtrip_snapshot() {
        roundtrip_response(&Response::Snapshot {
            snapshot: AnalyticsSnapshot {
                active_builds: HashMap::new(),
                recent_history: Vec::new(),
                machines: Vec::new(),
                dep_graphs: Vec::new(),
                build_processes: HashMap::new(),
            },
        });
    }

    #[test]
    fn response_roundtrip_builds() {
        roundtrip_response(&Response::Builds {
            builds: Vec::new(),
        });
    }

    #[test]
    fn response_roundtrip_build() {
        roundtrip_response(&Response::Build { build: None });
    }

    #[test]
    fn response_roundtrip_build_log() {
        roundtrip_response(&Response::BuildLog {
            activity_id: 1,
            lines: vec!["hello".to_string()],
        });
    }

    #[test]
    fn response_roundtrip_machines() {
        roundtrip_response(&Response::Machines {
            machines: Vec::new(),
        });
    }

    #[test]
    fn response_roundtrip_history() {
        roundtrip_response(&Response::History {
            builds: Vec::new(),
        });
    }

    #[test]
    fn response_roundtrip_ok() {
        roundtrip_response(&Response::Ok);
    }

    #[test]
    fn response_roundtrip_error() {
        roundtrip_response(&Response::Error {
            message: "something went wrong".to_string(),
        });
    }

    #[test]
    fn length_prefix_is_big_endian_u32() {
        let msg = Request::GetSnapshot;
        let encoded = encode_message(&msg).unwrap();
        let len_bytes: [u8; 4] = encoded[..4].try_into().unwrap();
        let len = u32::from_be_bytes(len_bytes);
        assert_eq!(len as usize, encoded.len() - 4);
    }

    #[test]
    fn invalid_msgpack_returns_error() {
        let garbage = b"this is not valid msgpack";
        let result = decode_message::<Request>(garbage);
        assert!(result.is_err());
    }

    #[test]
    fn socket_dir_from_env() {
        // Save and restore env to avoid poisoning other tests.
        let saved = std::env::var("NIX_ANALYTICS_SOCKET_DIR").ok();
        let saved_xdg = std::env::var("XDG_RUNTIME_DIR").ok();
        std::env::set_var("NIX_ANALYTICS_SOCKET_DIR", "/tmp/custom-analytics");
        assert_eq!(socket_dir(), PathBuf::from("/tmp/custom-analytics"));
        assert_eq!(
            event_socket_path(),
            PathBuf::from("/tmp/custom-analytics/events.sock")
        );
        assert_eq!(
            control_socket_path(),
            PathBuf::from("/tmp/custom-analytics/control.sock")
        );
        // Restore
        std::env::remove_var("NIX_ANALYTICS_SOCKET_DIR");
        match saved {
            Some(v) => std::env::set_var("NIX_ANALYTICS_SOCKET_DIR", v),
            None => {}
        }
        match saved_xdg {
            Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
            None => {}
        }
    }

    #[test]
    fn socket_dir_from_xdg_runtime_with_socket() {
        let saved = std::env::var("NIX_ANALYTICS_SOCKET_DIR").ok();
        let saved_xdg = std::env::var("XDG_RUNTIME_DIR").ok();

        let tmp = tempfile::tempdir().unwrap();
        let user_dir = tmp.path().join("nix-analytics");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::write(user_dir.join("control.sock"), b"").unwrap();

        std::env::remove_var("NIX_ANALYTICS_SOCKET_DIR");
        std::env::set_var("XDG_RUNTIME_DIR", tmp.path());
        assert_eq!(socket_dir(), user_dir);

        // Without a socket file, XDG is skipped
        std::fs::remove_file(user_dir.join("control.sock")).unwrap();
        assert_eq!(socket_dir(), PathBuf::from("/run/nix-analytics"));

        // Restore
        std::env::remove_var("XDG_RUNTIME_DIR");
        match saved {
            Some(v) => std::env::set_var("NIX_ANALYTICS_SOCKET_DIR", v),
            None => {}
        }
        match saved_xdg {
            Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
            None => {}
        }
    }

    #[test]
    fn socket_dir_default_fallback() {
        let saved = std::env::var("NIX_ANALYTICS_SOCKET_DIR").ok();
        let saved_xdg = std::env::var("XDG_RUNTIME_DIR").ok();
        std::env::remove_var("NIX_ANALYTICS_SOCKET_DIR");
        std::env::remove_var("XDG_RUNTIME_DIR");
        assert_eq!(socket_dir(), PathBuf::from("/run/nix-analytics"));
        // Restore
        match saved {
            Some(v) => std::env::set_var("NIX_ANALYTICS_SOCKET_DIR", v),
            None => {}
        }
        match saved_xdg {
            Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
            None => {}
        }
    }
}
