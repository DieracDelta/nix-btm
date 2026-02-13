//! Polls cgroup filesystem for per-build resource usage.
//!
//! Nix creates per-build cgroups named `nix-build-uid-<UID>` or
//! `nix-build-pid-<PID>-<N>` under the cgroup of the nix process that
//! started the build.  When builds go through nix-daemon, these appear under
//! the daemon's systemd service cgroup; when root runs nix directly, they
//! appear under the user's session scope.
//!
//! We discover the correct parent cgroup(s) by reading `/proc/<pid>/cgroup`
//! for each tracked nix process, then list `nix-build-*` subdirectories
//! in each parent.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use nix_analytics_common::types::ProcessInfo;

use crate::state::SharedState;

/// Interval between cgroup stat polls.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// The cgroup v2 mount point.
const CGROUP_BASE: &str = "/sys/fs/cgroup";

/// Poll loop that reads cgroup stats for active builds.
pub async fn poll_loop(state: SharedState) {
    let mut interval = tokio::time::interval(POLL_INTERVAL);

    loop {
        interval.tick().await;

        // Map each user_pid to its cgroup parent directory.
        let pid_parents = discover_pid_cgroup_parents(&state).await;

        // For each PID's cgroup parent, find nix-build-* subdirs and assign
        // them to all builds from that PID.
        let mut seen_parents = HashSet::new();
        for (pid, parent) in &pid_parents {
            let build_dirs = list_nix_build_dirs(parent).await;
            for path in &build_dirs {
                // Assign this cgroup to all builds with matching user_pid.
                state.assign_cgroup_by_pid(*pid, path.clone()).await;
            }
            seen_parents.insert(parent.clone());
        }

        // Also check the nix-daemon service cgroup as fallback.
        if let Some(svc) = get_daemon_service_cgroup().await {
            if seen_parents.insert(svc.clone()) {
                for path in list_nix_build_dirs(&svc).await {
                    state.assign_cgroup_to_any(&path).await;
                }
            }
        }

        // Update stats for all builds that have cgroups assigned.
        let cgroup_builds = state.all_cgroup_assignments().await;
        for (path, activity_ids) in &cgroup_builds {
            if let Some(stats) = read_cgroup_stats(path).await {
                for &activity_id in activity_ids {
                    state
                        .update_cgroup_stats(
                            activity_id,
                            stats.cpu_user_us,
                            stats.cpu_system_us,
                            stats.memory_current,
                            stats.is_frozen,
                        )
                        .await;
                }
            }

            // Read processes from cgroup.procs for each assigned cgroup.
            let pids = read_cgroup_procs(path).await;
            let mut procs = Vec::with_capacity(pids.len());
            for pid in pids {
                if let Some(info) = read_process_info(pid).await {
                    procs.push(info);
                }
            }
            for &activity_id in activity_ids {
                state
                    .update_build_processes(activity_id, procs.clone())
                    .await;
            }
        }
    }
}

/// Discover the cgroup parent directory for each active nix PID.
/// Returns (pid, parent_path) pairs.
async fn discover_pid_cgroup_parents(state: &SharedState) -> Vec<(u32, PathBuf)> {
    let pids = state.active_user_pids().await;
    let mut results = Vec::new();
    let mut seen = HashSet::new();

    for pid in pids {
        if let Some(cg) = read_proc_cgroup(pid).await {
            let parent = PathBuf::from(CGROUP_BASE).join(cg.trim_start_matches('/'));
            if seen.insert((pid, parent.clone())) {
                if tokio::fs::metadata(&parent).await.is_ok() {
                    tracing::debug!(?parent, pid, "scanning cgroup parent for nix-build dirs");
                    results.push((pid, parent));
                }
            }
        }
    }

    results
}

/// Read the cgroup v2 path for a given PID from `/proc/<pid>/cgroup`.
/// Returns the relative cgroup path (e.g. "/user.slice/user-0.slice/session-4.scope").
async fn read_proc_cgroup(pid: u32) -> Option<String> {
    let content = tokio::fs::read_to_string(format!("/proc/{pid}/cgroup"))
        .await
        .ok()?;
    // cgroup v2 format: "0::<path>"
    for line in content.lines() {
        if let Some(path) = line.strip_prefix("0::") {
            return Some(path.to_string());
        }
    }
    None
}

/// Query systemd for the nix-daemon service's cgroup path.
async fn get_daemon_service_cgroup() -> Option<PathBuf> {
    let output = tokio::process::Command::new("systemctl")
        .args(["show", "nix-daemon.service", "-p", "ControlGroup", "--value"])
        .output()
        .await
        .ok()?;

    let cgroup_rel = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if cgroup_rel.is_empty() {
        return None;
    }

    let path = PathBuf::from(CGROUP_BASE).join(cgroup_rel.trim_start_matches('/'));
    if tokio::fs::metadata(&path).await.is_ok() {
        Some(path)
    } else {
        None
    }
}

/// List `nix-build-*` subdirectories in a single directory.
async fn list_nix_build_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut results = Vec::new();
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(_) => return results,
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.file_name().to_string_lossy().starts_with("nix-build-") {
            let ft = match entry.file_type().await {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_dir() {
                results.push(entry.path());
            }
        }
    }
    results
}

struct CgroupStats {
    cpu_user_us: u64,
    cpu_system_us: u64,
    memory_current: Option<u64>,
    is_frozen: bool,
}

/// Read cpu.stat and memory.current from a cgroup directory.
async fn read_cgroup_stats(cgroup: &Path) -> Option<CgroupStats> {
    let cpu_stat = tokio::fs::read_to_string(cgroup.join("cpu.stat"))
        .await
        .ok()?;

    let mut cpu_user_us = 0u64;
    let mut cpu_system_us = 0u64;

    for line in cpu_stat.lines() {
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("user_usec") => {
                cpu_user_us = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            }
            Some("system_usec") => {
                cpu_system_us = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            }
            _ => {}
        }
    }

    let memory_current = tokio::fs::read_to_string(cgroup.join("memory.current"))
        .await
        .ok()
        .and_then(|s| s.trim().parse().ok());

    let is_frozen = tokio::fs::read_to_string(cgroup.join("cgroup.freeze"))
        .await
        .ok()
        .map(|s| s.trim() == "1")
        .unwrap_or(false);

    Some(CgroupStats {
        cpu_user_us,
        cpu_system_us,
        memory_current,
        is_frozen,
    })
}

/// Read the list of PIDs from a cgroup's `cgroup.procs` file.
async fn read_cgroup_procs(cgroup: &Path) -> Vec<u32> {
    let content = match tokio::fs::read_to_string(cgroup.join("cgroup.procs")).await {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    content
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .collect()
}

/// Read process info from /proc for a single PID.
async fn read_process_info(pid: u32) -> Option<ProcessInfo> {
    // Parse /proc/PID/stat for comm, ppid, state.
    let stat = tokio::fs::read_to_string(format!("/proc/{pid}/stat"))
        .await
        .ok()?;
    // Format: "PID (comm) S PPID ..."
    // comm can contain spaces and parens, so find the last ')'.
    let comm_start = stat.find('(')?;
    let comm_end = stat.rfind(')')?;
    let name = stat[comm_start + 1..comm_end].to_string();
    let rest = &stat[comm_end + 2..]; // skip ") "
    let mut fields = rest.split_whitespace();
    let state = fields.next()?.chars().next()?;
    let ppid: u32 = fields.next()?.parse().ok()?;

    // Read /proc/PID/cmdline for the full command.
    let cmdline = tokio::fs::read(format!("/proc/{pid}/cmdline"))
        .await
        .ok()
        .map(|bytes| {
            bytes
                .split(|&b| b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).to_string())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();

    // Read VmRSS from /proc/PID/status.
    let rss_bytes = tokio::fs::read_to_string(format!("/proc/{pid}/status"))
        .await
        .ok()
        .and_then(|status| {
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix("VmRSS:") {
                    let kb: u64 = rest.trim().split_whitespace().next()?.parse().ok()?;
                    return Some(kb * 1024);
                }
            }
            None
        })
        .unwrap_or(0);

    Some(ProcessInfo {
        pid,
        ppid,
        name,
        cmdline,
        state,
        rss_bytes,
    })
}
