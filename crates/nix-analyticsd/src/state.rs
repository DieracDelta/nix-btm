//! Shared mutable state for the analytics daemon.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use tokio::sync::RwLock;

use nix_analytics_common::event::{ActivityType, AnalyticsEvent};
use nix_analytics_common::protocol::{MAX_HISTORY, MAX_LOG_LINES};
use nix_analytics_common::types::{
    AnalyticsSnapshot, Build, BuildMachine, CompletedBuild, ProcessInfo, Progress, RemoteMachine,
};

use std::path::Path;

use crate::dep_graph::DepGraphManager;

/// Extract the UID from a cgroup directory name like `nix-build-uid-1000` or
/// `nix-build-uid-1000-0`.
fn parse_uid_from_cgroup_dir(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?;
    let rest = name.strip_prefix("nix-build-uid-")?;
    rest.split('-').next()?.parse().ok()
}

/// Thread-safe shared state handle.
#[derive(Clone)]
pub struct SharedState {
    inner: Arc<RwLock<State>>,
}

struct State {
    active_builds: HashMap<u64, Build>,
    history: VecDeque<CompletedBuild>,
    machines: Vec<RemoteMachine>,
    dep_graph_manager: DepGraphManager,
    build_processes: HashMap<u64, Vec<ProcessInfo>>,
}

impl SharedState {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(State {
                active_builds: HashMap::new(),
                history: VecDeque::with_capacity(MAX_HISTORY),
                machines: Vec::new(),
                dep_graph_manager: DepGraphManager::new(),
                build_processes: HashMap::new(),
            })),
        }
    }

    /// Process an incoming event from the plugin.
    pub async fn handle_event(&self, event: AnalyticsEvent) {
        let mut state = self.inner.write().await;
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
                // Clone for dep graph usage before moving into Build.
                let drv_path_copy = drv_path.clone();
                let command_line_copy = command_line.clone();

                let build = Build {
                    activity_id,
                    activity_type: activity_type.into(),
                    drv_path,
                    description,
                    parent_id: if parent_id == 0 {
                        None
                    } else {
                        Some(parent_id)
                    },
                    command_line,
                    started_at_us: timestamp_us,
                    phase: None,
                    recent_log: VecDeque::new(),
                    user: None,
                    user_pid: nix_pid,
                    user_uid: None,
                    cgroup_path: None,
                    cpu_user_us: 0,
                    cpu_system_us: 0,
                    memory_current: None,
                    progress: None,
                    machine: BuildMachine::Local,
                    is_frozen: false,
                };
                state.active_builds.insert(activity_id, build);

                // Wire into dependency graph for Build, Substitute, and Realise activities.
                // Resolving on Realise ensures the full tree is discovered early
                // (Nix starts Realise for the root derivation first).
                let activity_type_enum: ActivityType = activity_type.into();
                if matches!(activity_type_enum, ActivityType::Build | ActivityType::Substitute | ActivityType::Realise) {
                    if let Some(ref drv) = drv_path_copy {
                        // Find command_line by walking parent chain.
                        let cmd = command_line_copy.or_else(|| {
                            Self::find_command_line(&state.active_builds, parent_id)
                        });
                        if let Some(ref cmd) = cmd {
                            state.dep_graph_manager.resolve_drv_recursive(drv, cmd);
                            match activity_type_enum {
                                ActivityType::Build => {
                                    state.dep_graph_manager.set_building(
                                        activity_id, drv, cmd, timestamp_us,
                                    );
                                }
                                ActivityType::Substitute => {
                                    state.dep_graph_manager.set_substituting(
                                        activity_id, drv, cmd, timestamp_us,
                                    );
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }

            AnalyticsEvent::ActivityStopped {
                timestamp_us,
                activity_id,
            } => {
                state.build_processes.remove(&activity_id);
                if let Some(build) = state.active_builds.remove(&activity_id) {
                    let duration_us = timestamp_us.saturating_sub(build.started_at_us);
                    let success = build.progress.as_ref().is_none_or(|p| p.failed == 0);

                    // Update dep graph status.
                    if success {
                        state.dep_graph_manager.mark_done(activity_id);
                    } else {
                        state.dep_graph_manager.mark_failed(activity_id);
                    }

                    let completed = CompletedBuild {
                        cpu_user_total_us: build.cpu_user_us,
                        cpu_system_total_us: build.cpu_system_us,
                        build,
                        finished_at_us: timestamp_us,
                        duration: std::time::Duration::from_micros(duration_us),
                        success,
                    };
                    if state.history.len() >= MAX_HISTORY {
                        state.history.pop_front();
                    }
                    state.history.push_back(completed);
                }
            }

            AnalyticsEvent::PhaseChanged {
                activity_id, phase, ..
            } => {
                if let Some(build) = state.active_builds.get_mut(&activity_id) {
                    build.phase = Some(phase.clone());
                }
                state.dep_graph_manager.update_phase(activity_id, &phase);
            }

            AnalyticsEvent::LogLine {
                activity_id, text, ..
            } => {
                if let Some(build) = state.active_builds.get_mut(&activity_id) {
                    if build.recent_log.len() >= MAX_LOG_LINES {
                        build.recent_log.pop_front();
                    }
                    build.recent_log.push_back(text);
                }
            }

            AnalyticsEvent::Progress {
                activity_id,
                done,
                expected,
                running,
                failed,
                ..
            } => {
                if let Some(build) = state.active_builds.get_mut(&activity_id) {
                    build.progress = Some(Progress {
                        done,
                        expected,
                        running,
                        failed,
                    });
                }
                state.dep_graph_manager.update_progress(activity_id, done, expected);
            }

            AnalyticsEvent::PostBuildLogLine {
                activity_id, text, ..
            } => {
                if let Some(build) = state.active_builds.get_mut(&activity_id) {
                    if build.recent_log.len() >= MAX_LOG_LINES {
                        build.recent_log.pop_front();
                    }
                    build.recent_log.push_back(format!("[post-build-hook] {text}"));
                }
            }

            AnalyticsEvent::DrvCached {
                activity_id,
                drv_path,
                ..
            } => {
                // The plugin detected that a Realise stopped without a child
                // Build/Substitute, meaning the drv was already cached.
                // Walk the parent chain from the Realise activity to find the
                // command_line, resolve the drv into the graph, and mark it done.
                let cmd = state
                    .active_builds
                    .get(&activity_id)
                    .and_then(|b| {
                        b.command_line.clone().or_else(|| {
                            b.parent_id
                                .and_then(|pid| Self::find_command_line(&state.active_builds, pid))
                        })
                    });
                if let Some(cmd) = cmd {
                    state.dep_graph_manager.resolve_drv_recursive(&drv_path, &cmd);
                    state.dep_graph_manager.mark_cached(&drv_path, &cmd);
                }
            }

            AnalyticsEvent::RemoteDispatch {
                activity_id,
                machine_uri,
                ..
            } => {
                if let Some(build) = state.active_builds.get_mut(&activity_id) {
                    build.machine = BuildMachine::Remote {
                        uri: machine_uri.clone(),
                    };
                }
                // Also update the machine's active build list.
                for machine in &mut state.machines {
                    if machine.store_uri == machine_uri {
                        machine.active_build_ids.push(activity_id);
                    }
                }
            }
        }
    }

    /// Walk the parent_id chain to find the command_line for an activity.
    fn find_command_line(active_builds: &HashMap<u64, Build>, parent_id: u64) -> Option<String> {
        if parent_id == 0 {
            return None;
        }
        let mut current = parent_id;
        for _ in 0..50 {
            // depth limit to prevent infinite loops
            if let Some(build) = active_builds.get(&current) {
                if let Some(ref cmd) = build.command_line {
                    return Some(cmd.clone());
                }
                match build.parent_id {
                    Some(pid) if pid != 0 => current = pid,
                    _ => return None,
                }
            } else {
                return None;
            }
        }
        None
    }

    /// Get a snapshot of the current state for the TUI.
    pub async fn snapshot(&self) -> AnalyticsSnapshot {
        let state = self.inner.read().await;
        AnalyticsSnapshot {
            active_builds: state.active_builds.clone(),
            recent_history: state.history.iter().cloned().collect(),
            machines: state.machines.clone(),
            dep_graphs: state.dep_graph_manager.get_graphs(),
            build_processes: state.build_processes.clone(),
        }
    }

    /// Get a specific build by activity ID.
    pub async fn get_build(&self, activity_id: u64) -> Option<Build> {
        let state = self.inner.read().await;
        state.active_builds.get(&activity_id).cloned()
    }

    /// Get log lines for a build.
    pub async fn get_build_log(&self, activity_id: u64, last_n: usize) -> Vec<String> {
        let state = self.inner.read().await;
        if let Some(build) = state.active_builds.get(&activity_id) {
            build
                .recent_log
                .iter()
                .skip(build.recent_log.len().saturating_sub(last_n))
                .cloned()
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Get all active builds.
    pub async fn list_builds(&self) -> Vec<Build> {
        let state = self.inner.read().await;
        state.active_builds.values().cloned().collect()
    }

    /// Get remote machines.
    pub async fn list_machines(&self) -> Vec<RemoteMachine> {
        let state = self.inner.read().await;
        state.machines.clone()
    }

    /// Get build history.
    pub async fn get_history(&self, last_n: usize) -> Vec<CompletedBuild> {
        let state = self.inner.read().await;
        let start = state.history.len().saturating_sub(last_n);
        state.history.range(start..).cloned().collect()
    }

    /// Update machines list (called by the machine poller).
    pub async fn set_machines(&self, machines: Vec<RemoteMachine>) {
        let mut state = self.inner.write().await;
        state.machines = machines;
    }

    /// Update cgroup stats for a build.
    pub async fn update_cgroup_stats(
        &self,
        activity_id: u64,
        cpu_user_us: u64,
        cpu_system_us: u64,
        memory_current: Option<u64>,
        is_frozen: bool,
    ) {
        let mut state = self.inner.write().await;
        if let Some(build) = state.active_builds.get_mut(&activity_id) {
            build.cpu_user_us = cpu_user_us;
            build.cpu_system_us = cpu_system_us;
            build.memory_current = memory_current;
            build.is_frozen = is_frozen;
        }
    }

    /// Update the list of processes running inside a build's cgroup.
    pub async fn update_build_processes(&self, activity_id: u64, processes: Vec<ProcessInfo>) {
        let mut state = self.inner.write().await;
        if state.active_builds.contains_key(&activity_id) {
            state.build_processes.insert(activity_id, processes);
        }
    }

    /// Set the cgroup path for a build, parsing UID from the cgroup dir name if possible.
    pub async fn set_cgroup_path(&self, activity_id: u64, path: std::path::PathBuf) {
        let mut state = self.inner.write().await;
        if let Some(build) = state.active_builds.get_mut(&activity_id) {
            // Parse UID from cgroup name like "nix-build-uid-1000-0"
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if let Some(rest) = name.strip_prefix("nix-build-uid-") {
                    if let Some(uid_str) = rest.split('-').next() {
                        if let Ok(uid) = uid_str.parse::<u32>() {
                            build.user_uid = Some(uid);
                        }
                    }
                }
            }
            build.cgroup_path = Some(path);
        }
    }

    /// Assign a cgroup path to all active builds with the given user_pid that
    /// don't already have a cgroup.  Also parses the UID from the cgroup dir name.
    pub async fn assign_cgroup_by_pid(&self, pid: u32, path: std::path::PathBuf) {
        let uid = parse_uid_from_cgroup_dir(&path);
        let mut state = self.inner.write().await;
        for build in state.active_builds.values_mut() {
            if build.cgroup_path.is_none() && build.user_pid == Some(pid) {
                tracing::info!(activity_id = build.activity_id, ?path, pid, "assigning cgroup by PID");
                if let Some(uid) = uid {
                    build.user_uid = Some(uid);
                }
                build.cgroup_path = Some(path.clone());
            }
        }
    }

    /// Assign a cgroup to any unassigned build (fallback for daemon-socket builds).
    pub async fn assign_cgroup_to_any(&self, path: &std::path::Path) {
        let uid = parse_uid_from_cgroup_dir(path);
        let mut state = self.inner.write().await;
        for build in state.active_builds.values_mut() {
            if build.cgroup_path.is_none() {
                tracing::info!(activity_id = build.activity_id, ?path, "assigning cgroup (fallback)");
                if let Some(uid) = uid {
                    build.user_uid = Some(uid);
                }
                build.cgroup_path = Some(path.to_path_buf());
            }
        }
    }

    /// Get all cgroup→activity_id mappings for stats updates.
    pub async fn all_cgroup_assignments(&self) -> HashMap<std::path::PathBuf, Vec<u64>> {
        let state = self.inner.read().await;
        let mut map: HashMap<std::path::PathBuf, Vec<u64>> = HashMap::new();
        for build in state.active_builds.values() {
            if let Some(ref path) = build.cgroup_path {
                map.entry(path.clone()).or_default().push(build.activity_id);
            }
        }
        map
    }

    /// Get activity IDs of builds that have no cgroup assigned, sorted by start time (newest first).
    pub async fn builds_without_cgroup(&self) -> Vec<u64> {
        let state = self.inner.read().await;
        let mut builds: Vec<_> = state
            .active_builds
            .values()
            .filter(|b| b.cgroup_path.is_none())
            .map(|b| (b.activity_id, b.started_at_us))
            .collect();
        builds.sort_by(|a, b| b.1.cmp(&a.1)); // newest first
        builds.into_iter().map(|(id, _)| id).collect()
    }

    /// Get the cgroup path for a build (used by control actions).
    pub async fn get_cgroup_path(&self, activity_id: u64) -> Option<std::path::PathBuf> {
        let state = self.inner.read().await;
        state
            .active_builds
            .get(&activity_id)
            .and_then(|b| b.cgroup_path.clone())
    }

    /// Get the nix daemon fork PID for a build (used as fallback when cgroups are unavailable).
    pub async fn get_user_pid(&self, activity_id: u64) -> Option<u32> {
        let state = self.inner.read().await;
        state
            .active_builds
            .get(&activity_id)
            .and_then(|b| b.user_pid)
    }

    /// Set the frozen state for a build.
    pub async fn set_frozen(&self, activity_id: u64, frozen: bool) {
        let mut state = self.inner.write().await;
        if let Some(build) = state.active_builds.get_mut(&activity_id) {
            build.is_frozen = frozen;
        }
    }

    /// Get unique PIDs of active nix processes (for cgroup parent discovery).
    pub async fn active_user_pids(&self) -> Vec<u32> {
        let state = self.inner.read().await;
        let mut pids: Vec<u32> = state
            .active_builds
            .values()
            .filter_map(|b| b.user_pid)
            .collect();
        pids.sort_unstable();
        pids.dedup();
        pids
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_started_event(id: u64, ts: u64) -> AnalyticsEvent {
        AnalyticsEvent::ActivityStarted {
            timestamp_us: ts,
            activity_id: id,
            activity_type: 105,
            description: format!("building test-{id}"),
            drv_path: Some(format!("/nix/store/abc-test-{id}.drv")),
            parent_id: 0,
            command_line: None,
            nix_pid: None,
        }
    }

    #[tokio::test]
    async fn activity_started_creates_build() {
        let state = SharedState::new();
        state.handle_event(make_started_event(1, 1000)).await;
        let builds = state.list_builds().await;
        assert_eq!(builds.len(), 1);
        assert_eq!(builds[0].activity_id, 1);
        assert_eq!(builds[0].description, "building test-1");
    }

    #[tokio::test]
    async fn activity_stopped_moves_to_history() {
        let state = SharedState::new();
        state.handle_event(make_started_event(1, 1000)).await;
        state
            .handle_event(AnalyticsEvent::ActivityStopped {
                timestamp_us: 2000,
                activity_id: 1,
            })
            .await;

        assert!(state.list_builds().await.is_empty());
        let history = state.get_history(10).await;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].build.activity_id, 1);
        assert_eq!(history[0].finished_at_us, 2000);
    }

    #[tokio::test]
    async fn success_detection_no_progress() {
        let state = SharedState::new();
        state.handle_event(make_started_event(1, 1000)).await;
        state
            .handle_event(AnalyticsEvent::ActivityStopped {
                timestamp_us: 2000,
                activity_id: 1,
            })
            .await;
        let history = state.get_history(10).await;
        assert!(history[0].success);
    }

    #[tokio::test]
    async fn success_detection_failed_zero() {
        let state = SharedState::new();
        state.handle_event(make_started_event(1, 1000)).await;
        state
            .handle_event(AnalyticsEvent::Progress {
                timestamp_us: 1500,
                activity_id: 1,
                done: 5,
                expected: 10,
                running: 1,
                failed: 0,
            })
            .await;
        state
            .handle_event(AnalyticsEvent::ActivityStopped {
                timestamp_us: 2000,
                activity_id: 1,
            })
            .await;
        let history = state.get_history(10).await;
        assert!(history[0].success);
    }

    #[tokio::test]
    async fn success_detection_failed_nonzero() {
        let state = SharedState::new();
        state.handle_event(make_started_event(1, 1000)).await;
        state
            .handle_event(AnalyticsEvent::Progress {
                timestamp_us: 1500,
                activity_id: 1,
                done: 5,
                expected: 10,
                running: 1,
                failed: 3,
            })
            .await;
        state
            .handle_event(AnalyticsEvent::ActivityStopped {
                timestamp_us: 2000,
                activity_id: 1,
            })
            .await;
        let history = state.get_history(10).await;
        assert!(!history[0].success);
    }

    #[tokio::test]
    async fn phase_changed_updates_build() {
        let state = SharedState::new();
        state.handle_event(make_started_event(1, 1000)).await;
        state
            .handle_event(AnalyticsEvent::PhaseChanged {
                timestamp_us: 1500,
                activity_id: 1,
                phase: "buildPhase".to_string(),
            })
            .await;
        let build = state.get_build(1).await.unwrap();
        assert_eq!(build.phase.as_deref(), Some("buildPhase"));
    }

    #[tokio::test]
    async fn log_line_appends_to_recent_log() {
        let state = SharedState::new();
        state.handle_event(make_started_event(1, 1000)).await;
        state
            .handle_event(AnalyticsEvent::LogLine {
                timestamp_us: 1500,
                activity_id: 1,
                text: "compiling main.c".to_string(),
            })
            .await;
        let log = state.get_build_log(1, 100).await;
        assert_eq!(log, vec!["compiling main.c"]);
    }

    #[tokio::test]
    async fn log_line_truncates_at_max() {
        let state = SharedState::new();
        state.handle_event(make_started_event(1, 1000)).await;
        for i in 0..MAX_LOG_LINES + 10 {
            state
                .handle_event(AnalyticsEvent::LogLine {
                    timestamp_us: 1500 + i as u64,
                    activity_id: 1,
                    text: format!("line {i}"),
                })
                .await;
        }
        let log = state.get_build_log(1, MAX_LOG_LINES + 100).await;
        assert_eq!(log.len(), MAX_LOG_LINES);
        // Oldest lines should have been evicted.
        assert_eq!(log[0], format!("line {}", 10));
    }

    #[tokio::test]
    async fn post_build_log_line_prefixed() {
        let state = SharedState::new();
        state.handle_event(make_started_event(1, 1000)).await;
        state
            .handle_event(AnalyticsEvent::PostBuildLogLine {
                timestamp_us: 1500,
                activity_id: 1,
                text: "signing path".to_string(),
            })
            .await;
        let log = state.get_build_log(1, 100).await;
        assert_eq!(log, vec!["[post-build-hook] signing path"]);
    }

    #[tokio::test]
    async fn progress_updates_counters() {
        let state = SharedState::new();
        state.handle_event(make_started_event(1, 1000)).await;
        state
            .handle_event(AnalyticsEvent::Progress {
                timestamp_us: 1500,
                activity_id: 1,
                done: 3,
                expected: 10,
                running: 2,
                failed: 1,
            })
            .await;
        let build = state.get_build(1).await.unwrap();
        let progress = build.progress.unwrap();
        assert_eq!(progress.done, 3);
        assert_eq!(progress.expected, 10);
        assert_eq!(progress.running, 2);
        assert_eq!(progress.failed, 1);
    }

    #[tokio::test]
    async fn remote_dispatch_sets_machine() {
        let state = SharedState::new();
        state.handle_event(make_started_event(1, 1000)).await;

        // Pre-seed a machine so the active_build_ids update works.
        state
            .set_machines(vec![RemoteMachine {
                store_uri: "ssh-ng://builder".to_string(),
                system_types: vec!["x86_64-linux".to_string()],
                max_jobs: 4,
                speed_factor: 1.0,
                supported_features: Vec::new(),
                mandatory_features: Vec::new(),
                active_slots: 0,
                active_build_ids: Vec::new(),
            }])
            .await;

        state
            .handle_event(AnalyticsEvent::RemoteDispatch {
                timestamp_us: 1500,
                activity_id: 1,
                machine_uri: "ssh-ng://builder".to_string(),
            })
            .await;

        let build = state.get_build(1).await.unwrap();
        match &build.machine {
            BuildMachine::Remote { uri } => assert_eq!(uri, "ssh-ng://builder"),
            _ => panic!("expected Remote"),
        }

        let machines = state.list_machines().await;
        assert!(machines[0].active_build_ids.contains(&1));
    }

    #[tokio::test]
    async fn history_capped_at_max() {
        let state = SharedState::new();
        for i in 0..(MAX_HISTORY as u64 + 5) {
            state.handle_event(make_started_event(i, i * 1000)).await;
            state
                .handle_event(AnalyticsEvent::ActivityStopped {
                    timestamp_us: i * 1000 + 500,
                    activity_id: i,
                })
                .await;
        }
        let history = state.get_history(MAX_HISTORY + 100).await;
        assert_eq!(history.len(), MAX_HISTORY);
    }

    #[tokio::test]
    async fn set_cgroup_path_parses_uid() {
        let state = SharedState::new();
        state.handle_event(make_started_event(1, 1000)).await;
        state
            .set_cgroup_path(1, PathBuf::from("/sys/fs/cgroup/nix-daemon/nix-build-uid-1000-0"))
            .await;
        let build = state.get_build(1).await.unwrap();
        assert_eq!(build.user_uid, Some(1000));
        assert!(build.cgroup_path.is_some());
    }

    #[tokio::test]
    async fn builds_without_cgroup_returns_newest_first() {
        let state = SharedState::new();
        state.handle_event(make_started_event(1, 1000)).await;
        state.handle_event(make_started_event(2, 2000)).await;
        state.handle_event(make_started_event(3, 3000)).await;

        // Assign cgroup to build 2 only.
        state.set_cgroup_path(2, PathBuf::from("/tmp/cg")).await;

        let without = state.builds_without_cgroup().await;
        assert_eq!(without, vec![3, 1]); // newest first
    }

    #[tokio::test]
    async fn update_cgroup_stats_updates_fields() {
        let state = SharedState::new();
        state.handle_event(make_started_event(1, 1000)).await;
        state.update_cgroup_stats(1, 500, 200, Some(1024), false).await;
        let build = state.get_build(1).await.unwrap();
        assert_eq!(build.cpu_user_us, 500);
        assert_eq!(build.cpu_system_us, 200);
        assert_eq!(build.memory_current, Some(1024));
        assert!(!build.is_frozen);
    }

    #[tokio::test]
    async fn update_cgroup_stats_sets_frozen() {
        let state = SharedState::new();
        state.handle_event(make_started_event(1, 1000)).await;
        state.update_cgroup_stats(1, 0, 0, None, true).await;
        let build = state.get_build(1).await.unwrap();
        assert!(build.is_frozen);
    }

    #[tokio::test]
    async fn get_build_returns_none_for_missing() {
        let state = SharedState::new();
        assert!(state.get_build(999).await.is_none());
    }

    #[tokio::test]
    async fn activity_started_stores_nix_pid() {
        let state = SharedState::new();
        state
            .handle_event(AnalyticsEvent::ActivityStarted {
                timestamp_us: 1000,
                activity_id: 42,
                activity_type: 105,
                description: "building test-42".to_string(),
                drv_path: Some("/nix/store/abc-test-42.drv".to_string()),
                parent_id: 0,
                command_line: None,
                nix_pid: Some(12345),
            })
            .await;
        assert_eq!(state.get_user_pid(42).await, Some(12345));
    }
}
