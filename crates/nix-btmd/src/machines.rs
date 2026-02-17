//! Polls remote builder configuration and slot utilization.
//!
//! Reads the same data sources as Nix's build-remote:
//! - /etc/nix/machines for builder configuration
//! - /nix/var/nix/current-load/ for slot lock files (load tracking)

use std::path::Path;

use nix_btm_common::types::RemoteMachine;

use crate::state::SharedState;

/// Interval between machine status polls.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Where Nix stores the machine configuration.
const MACHINES_FILE: &str = "/etc/nix/machines";

/// Where Nix stores slot lock files for load tracking.
const CURRENT_LOAD_DIR: &str = "/nix/var/nix/current-load";

/// Poll loop that reads machine config and slot utilization.
pub async fn poll_loop(state: SharedState) {
    let mut interval = tokio::time::interval(POLL_INTERVAL);

    loop {
        interval.tick().await;

        let machines = match read_machines().await {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!("could not read machines: {e}");
                continue;
            }
        };

        state.set_machines(machines).await;
    }
}

/// Parse /etc/nix/machines and check slot utilization.
async fn read_machines() -> anyhow::Result<Vec<RemoteMachine>> {
    let content = tokio::fs::read_to_string(MACHINES_FILE).await?;
    let mut machines = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Handle @file includes.
        if let Some(path) = line.strip_prefix('@') {
            if let Ok(included) = tokio::fs::read_to_string(path.trim()).await {
                for inc_line in included.lines() {
                    if let Some(m) = parse_machine_line(inc_line) {
                        machines.push(m);
                    }
                }
            }
            continue;
        }

        if let Some(m) = parse_machine_line(line) {
            machines.push(m);
        }
    }

    // Check slot utilization from lock files.
    for machine in &mut machines {
        machine.active_slots = count_active_slots(&machine.store_uri, machine.max_jobs).await;
    }

    Ok(machines)
}

/// Parse a single machine line from /etc/nix/machines.
///
/// Format: storeUri systemTypes sshKey maxJobs speedFactor supportedFeatures mandatoryFeatures sshPublicHostKey
/// Fields after storeUri are optional. A `-` means default.
fn parse_machine_line(line: &str) -> Option<RemoteMachine> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.is_empty() {
        return None;
    }

    let store_uri = fields[0].to_string();

    let system_types = fields
        .get(1)
        .filter(|s| **s != "-")
        .map(|s| s.split(',').map(String::from).collect())
        .unwrap_or_default();

    // fields[2] is sshKey, skip for our purposes.

    let max_jobs = fields
        .get(3)
        .filter(|s| **s != "-")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    let speed_factor = fields
        .get(4)
        .filter(|s| **s != "-")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);

    let supported_features = fields
        .get(5)
        .filter(|s| **s != "-")
        .map(|s| s.split(',').map(String::from).collect())
        .unwrap_or_default();

    let mandatory_features = fields
        .get(6)
        .filter(|s| **s != "-")
        .map(|s| s.split(',').map(String::from).collect())
        .unwrap_or_default();

    Some(RemoteMachine {
        store_uri,
        system_types,
        max_jobs,
        speed_factor,
        supported_features,
        mandatory_features,
        active_slots: 0,
        active_build_ids: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_machine_line() {
        let line = "ssh-ng://builder x86_64-linux,aarch64-linux /etc/nix/key 4 2.0 big-parallel,nixos-test kvm,benchmark";
        let m = parse_machine_line(line).unwrap();
        assert_eq!(m.store_uri, "ssh-ng://builder");
        assert_eq!(m.system_types, vec!["x86_64-linux", "aarch64-linux"]);
        assert_eq!(m.max_jobs, 4);
        assert_eq!(m.speed_factor, 2.0);
        assert_eq!(m.supported_features, vec!["big-parallel", "nixos-test"]);
        assert_eq!(m.mandatory_features, vec!["kvm", "benchmark"]);
    }

    #[test]
    fn parse_minimal_machine_line() {
        let line = "ssh-ng://builder";
        let m = parse_machine_line(line).unwrap();
        assert_eq!(m.store_uri, "ssh-ng://builder");
        assert!(m.system_types.is_empty());
        assert_eq!(m.max_jobs, 1);
        assert_eq!(m.speed_factor, 1.0);
        assert!(m.supported_features.is_empty());
        assert!(m.mandatory_features.is_empty());
    }

    #[test]
    fn parse_dashes_as_defaults() {
        let line = "ssh-ng://builder - - - - - -";
        let m = parse_machine_line(line).unwrap();
        assert_eq!(m.store_uri, "ssh-ng://builder");
        assert!(m.system_types.is_empty());
        assert_eq!(m.max_jobs, 1);
        assert_eq!(m.speed_factor, 1.0);
        assert!(m.supported_features.is_empty());
        assert!(m.mandatory_features.is_empty());
    }

    #[test]
    fn parse_empty_line_returns_none() {
        assert!(parse_machine_line("").is_none());
        assert!(parse_machine_line("   ").is_none());
    }

    #[test]
    fn parse_comment_returns_none() {
        assert!(parse_machine_line("# a comment").is_none());
        assert!(parse_machine_line("  # indented comment").is_none());
    }

    #[test]
    fn parse_multiple_system_types_and_features() {
        let line = "ssh://host aarch64-linux,x86_64-linux - 8 1.5 feat1,feat2,feat3 mfeat1,mfeat2";
        let m = parse_machine_line(line).unwrap();
        assert_eq!(m.system_types, vec!["aarch64-linux", "x86_64-linux"]);
        assert_eq!(m.supported_features, vec!["feat1", "feat2", "feat3"]);
        assert_eq!(m.mandatory_features, vec!["mfeat1", "mfeat2"]);
    }
}

/// Count active slots for a machine by checking lock files.
///
/// Nix's build-remote creates lock files at:
///   /nix/var/nix/current-load/<escaped-uri>-<slot>
async fn count_active_slots(store_uri: &str, max_jobs: u32) -> u32 {
    let load_dir = Path::new(CURRENT_LOAD_DIR);
    if !load_dir.exists() {
        return 0;
    }

    let escaped_uri = store_uri.replace('/', "_");
    let mut active = 0;

    for slot in 0..max_jobs {
        let lock_path = load_dir.join(format!("{escaped_uri}-{slot}"));
        if lock_path.exists() {
            // Try to check if the file is locked (indicates active build).
            // A simple heuristic: if the file exists and was recently modified,
            // it's likely locked.
            if let Ok(meta) = tokio::fs::metadata(&lock_path).await {
                if let Ok(modified) = meta.modified() {
                    if let Ok(elapsed) = modified.elapsed() {
                        // If modified in the last hour, consider it active.
                        if elapsed.as_secs() < 3600 {
                            active += 1;
                        }
                    }
                }
            }
        }
    }

    active
}
