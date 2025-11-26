use std::{
    collections::{BTreeMap, HashMap, HashSet, hash_map::Entry},
    fmt::Display,
    fs,
    io::{self},
    ops::Deref,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use bstr::ByteSlice as _;
use either::Either;
use futures::FutureExt;
use json_parsing_nix::{ActivityType, Field, LogMessage, VerbosityLevel};
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    net::{UnixListener, UnixStream},
    sync::{
        RwLock,
        mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
        watch,
    },
    time::{MissedTickBehavior, interval},
};
use tracing::error;

use crate::{
    derivation_tree::{DrvRelations, START_INSTANT},
    shutdown::Shutdown,
    spawn_named,
};

#[repr(transparent)]
#[derive(
    Copy,
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
)]
#[serde(transparent)]
pub struct JobId(pub u64);

impl From<u64> for JobId {
    fn from(value: u64) -> Self {
        JobId(value)
    }
}

impl Deref for JobId {
    type Target = u64;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Unique identifier for a build target
#[repr(transparent)]
#[derive(
    Copy,
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    Default,
)]
#[serde(transparent)]
pub struct BuildTargetId(pub u64);

impl From<u64> for BuildTargetId {
    fn from(value: u64) -> Self {
        BuildTargetId(value)
    }
}

impl Deref for BuildTargetId {
    type Target = u64;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Status of a build target (high-level view of all jobs for a target)
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetStatus {
    /// Evaluation in progress (parsing flake, computing dependencies)
    Evaluating,
    /// Queued to build (dep tree known, but no active jobs yet)
    Queued,
    /// At least one job is actively building/downloading
    Active,
    /// All jobs completed successfully
    Completed,
    /// All drvs were already built (from cache, no actual work needed)
    Cached,
    /// Build was cancelled by user
    Cancelled,
}

impl Display for TargetStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetStatus::Evaluating => write!(f, "Evaluating"),
            TargetStatus::Queued => write!(f, "Queued"),
            TargetStatus::Active => write!(f, "Active"),
            TargetStatus::Completed => write!(f, "Completed"),
            TargetStatus::Cached => write!(f, "Cached"),
            TargetStatus::Cancelled => write!(f, "Cancelled"),
        }
    }
}

/// A build target represents a user's build request (e.g., "nix build
/// nixpkgs#bat")
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuildTarget {
    /// Unique identifier for this target
    pub id: BuildTargetId,

    /// Human-readable flake reference (e.g., "github:nixos/nixpkgs#bat")
    pub reference: String,

    /// The top-level drv for this target (from `nix eval 'target.drvPath'`)
    pub root_drv: Drv,

    /// All drvs in the transitive dependency closure
    /// Computed once when target is discovered via `nix derivation show`
    pub transitive_closure: HashSet<Drv>,

    /// Which requester (build session) owns this target
    pub requester_id: RequesterId,

    /// Current status of the target
    pub status: TargetStatus,
}

impl BuildTarget {
    /// Compute the target's status from the jobs in state
    /// This should be called whenever jobs change
    pub fn compute_status(
        &self,
        jid_to_job: &HashMap<JobId, BuildJob>,
        drv_to_jobs: &HashMap<Drv, HashSet<JobId>>,
        already_built_drvs: &HashSet<Drv>,
    ) -> TargetStatus {
        // Collect all jobs for drvs in this target's closure
        let mut all_jobs = Vec::new();
        for drv in &self.transitive_closure {
            if let Some(jobs) = drv_to_jobs.get(drv) {
                for jid in jobs {
                    if let Some(job) = jid_to_job.get(jid) {
                        // Only consider jobs from this requester
                        if job.rid == self.requester_id {
                            all_jobs.push(job);
                        }
                    }
                }
            }
        }

        // If no jobs exist yet, check if everything is already built
        if all_jobs.is_empty() {
            let all_already_built = self
                .transitive_closure
                .iter()
                .all(|drv| already_built_drvs.contains(drv));

            if all_already_built && !self.transitive_closure.is_empty() {
                return TargetStatus::Cached;
            } else {
                return TargetStatus::Queued;
            }
        }

        // Check for evaluation/fetching activities
        if all_jobs.iter().any(|j| {
            matches!(
                j.status,
                JobStatus::Evaluating | JobStatus::FetchingTree(_)
            )
        }) {
            return TargetStatus::Evaluating;
        }

        // Check if any jobs are actively running
        if all_jobs.iter().any(|j| j.status.is_active()) {
            return TargetStatus::Active;
        }

        // Check if cancelled
        if all_jobs.iter().any(|j| j.status == JobStatus::Cancelled) {
            return TargetStatus::Cancelled;
        }

        // Check if all completed
        if all_jobs.iter().all(|j| j.status.is_completed()) {
            return TargetStatus::Completed;
        }

        // Default to queued
        TargetStatus::Queued
    }
}

#[derive(Clone, Debug, Default)]
pub struct JobsStateInner {
    /// All known build targets, indexed by unique ID
    /// BTreeMap for stable iteration order
    pub targets: BTreeMap<BuildTargetId, BuildTarget>,

    /// Reverse index: which targets contain each drv
    /// Multiple targets can share drvs (common dependencies)
    pub drv_to_targets: HashMap<Drv, HashSet<BuildTargetId>>,

    /// Next target ID to assign (monotonically increasing)
    pub next_target_id: BuildTargetId,

    /// All jobs, indexed by job ID
    pub jid_to_job: HashMap<JobId, BuildJob>,

    /// Reverse index: which jobs are building each drv
    pub drv_to_jobs: HashMap<Drv, HashSet<JobId>>,

    /// Dependency tree of all known derivations
    pub dep_tree: DrvRelations,

    /// Drvs that were already built (cached) - for status display
    pub already_built_drvs: HashSet<Drv>,
}

impl JobsStateInner {
    /// Get global status of a drv (not target-specific)
    /// For target-specific status, use get_drv_status_for_target instead
    pub fn get_status(&self, drv: &Drv) -> JobStatus {
        // Check if this drv has explicit jobs
        if let Some(jobs) = self.drv_to_jobs.get(drv) {
            for job in jobs {
                if let Some(bj) = self.jid_to_job.get(job) {
                    return bj.status.clone();
                }
            }
        }

        // Check if this drv was already built (cached)
        if self.already_built_drvs.contains(drv) {
            return JobStatus::AlreadyBuilt;
        }

        // If drv is in the dependency tree, it's queued to be built
        if self.dep_tree.nodes.contains_key(drv) {
            JobStatus::Queued
        } else {
            JobStatus::NotEnoughInfo
        }
    }

    pub fn make_tree_description(&self, drv: &Drv) -> String {
        let status = self.get_status(drv);
        // Get required outputs for this drv if available
        let outputs_str = if let Some(node) = self.dep_tree.nodes.get(drv) {
            if node.required_outputs.is_empty() {
                String::new()
            } else {
                let outputs: Vec<&str> =
                    node.required_outputs.iter().map(|s| s.as_str()).collect();
                format!(" [{}]", outputs.join(", "))
            }
        } else {
            String::new()
        };
        format!(
            "{} - {} - {}{}",
            drv.name.clone(),
            drv.hash.clone(),
            status,
            outputs_str
        )
    }

    /// Create a new build target and return its ID
    pub fn create_target(
        &mut self,
        reference: String,
        root_drv: Drv,
        requester_id: RequesterId,
    ) -> BuildTargetId {
        let target_id = self.next_target_id;
        self.next_target_id = (*self.next_target_id + 1).into();

        // Compute transitive closure from dep_tree
        let transitive_closure = self.compute_transitive_closure(&root_drv);

        let target = BuildTarget {
            id: target_id,
            reference,
            root_drv,
            transitive_closure,
            requester_id,
            status: TargetStatus::Evaluating,
        };

        // Update reverse index: map each drv to this target
        for drv in &target.transitive_closure {
            self.drv_to_targets
                .entry(drv.clone())
                .or_default()
                .insert(target_id);
        }

        self.targets.insert(target_id, target);
        target_id
    }

    /// Compute transitive closure of a drv from the dep_tree
    fn compute_transitive_closure(&self, root: &Drv) -> HashSet<Drv> {
        let mut closure = HashSet::new();
        let mut stack = vec![root.clone()];

        while let Some(drv) = stack.pop() {
            if closure.insert(drv.clone()) {
                if let Some(node) = self.dep_tree.nodes.get(&drv) {
                    for dep in &node.deps {
                        stack.push(dep.clone());
                    }
                }
            }
        }

        closure
    }

    /// Update the status of a target based on current job state
    pub fn update_target_status(&mut self, target_id: BuildTargetId) {
        if let Some(target) = self.targets.get_mut(&target_id) {
            let new_status = target.compute_status(
                &self.jid_to_job,
                &self.drv_to_jobs,
                &self.already_built_drvs,
            );
            target.status = new_status;
        }
    }

    /// Get the status of a drv within a specific target's context
    /// This allows different targets to have different views of the same drv
    pub fn get_drv_status_for_target(
        &self,
        drv: &Drv,
        target_id: BuildTargetId,
    ) -> JobStatus {
        // Get the target to find which requester owns it
        let Some(target) = self.targets.get(&target_id) else {
            return JobStatus::NotEnoughInfo;
        };

        // Check if this drv has jobs from this target's requester
        if let Some(jobs) = self.drv_to_jobs.get(drv) {
            for job in jobs {
                if let Some(bj) = self.jid_to_job.get(job) {
                    if bj.rid == target.requester_id {
                        return bj.status.clone();
                    }
                }
            }
        }

        // Check if this drv was already built (cached)
        if self.already_built_drvs.contains(drv) {
            return JobStatus::AlreadyBuilt;
        }

        // Check target status for cancelled
        if target.status == TargetStatus::Cancelled {
            return JobStatus::Cancelled;
        }

        // If drv is in the dependency tree, it's queued to be built
        if self.dep_tree.nodes.contains_key(drv) {
            JobStatus::Queued
        } else {
            JobStatus::NotEnoughInfo
        }
    }

    /// Get all targets for a given requester
    pub fn get_targets_for_requester(
        &self,
        rid: RequesterId,
    ) -> Vec<&BuildTarget> {
        self.targets
            .values()
            .filter(|t| t.requester_id == rid)
            .collect()
    }
}

/// Evaluate a flake reference to get its .drv path
async fn eval_flake_to_drv(flake_ref: &str) -> Option<Drv> {
    use tokio::process::Command;

    // Run: nix eval --raw 'flake_ref.drvPath'
    let output = Command::new("nix")
        .arg("eval")
        .arg("--raw")
        .arg(format!("{}.drvPath", flake_ref))
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        tracing::debug!(
            "failed to eval {}: {:?}",
            flake_ref,
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }

    let drv_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    match parse_store_path(&drv_path) {
        Either::Left(drv) => {
            tracing::info!("evaluated {} -> {}", flake_ref, drv.name);
            Some(drv)
        }
        Either::Right(_) => {
            tracing::debug!(
                "{} didn't evaluate to a .drv path: {}",
                flake_ref,
                drv_path
            );
            None
        }
    }
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Hash,
    Eq,
    Default,
    Ord,
    PartialOrd,
    Serialize,
    Deserialize,
)]
#[serde(
    try_from = "crate::protocol_common::DrvWire",
    into = "crate::protocol_common::DrvWire"
)]
pub struct Drv {
    pub name: String,
    pub hash: String,
}

impl Display for Drv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}.drv", self.hash, self.name)
    }
}

impl Display for DrvParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error parsing drv")
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DrvParseError;

impl FromStr for Drv {
    type Err = DrvParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match parse_store_path(s.trim()) {
            Either::Left(drv) => Ok(drv),
            Either::Right(_) => Err(DrvParseError),
        }
    }
}

//impl Serialize for Drv {
//    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
//        ser.serialize_str(&self.to_string())
//    }
//}
//
//impl<'de> Deserialize<'de> for Drv {
//    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
//        let s = String::deserialize(de)?;
//        s.parse().map_err(|_| D::Error::custom("not a .drv path"))
//    }
//}

#[derive(
    Clone, Debug, PartialEq, Hash, Eq, Default, Deserialize, Ord, PartialOrd,
)]
pub struct StoreOutput {
    pub name: String,
    pub hash: String,
}

#[derive(Clone, Debug, Default)]
pub struct JobsState(Arc<RwLock<JobsStateInner>>);

impl Deref for JobsState {
    type Target = Arc<RwLock<JobsStateInner>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl JobsState {
    pub async fn stop_build_job(&self, id: JobId) {
        let mut state = self.write().await;
        match state.jid_to_job.entry(id) {
            Entry::Occupied(mut occupied_entry) => {
                let job = occupied_entry.get_mut();
                job.status = job.status.mark_complete();
                job.stop_time_ns =
                    Some(START_INSTANT.elapsed().as_nanos() as u64);
            }
            Entry::Vacant(_vacant_entry) => {}
        }
    }
    pub async fn insert_idle_drv(&self, drv: Drv) {
        let mut state = self.write().await;
        // Only query Nix for real derivations (32-char base32 hashes)
        if drv.hash.len() == 32 {
            state.dep_tree.insert(drv.clone()).await;
        }
    }

    /// Insert an idle drv and track which requester it belongs to
    /// Used for top-level drvs from flake evaluation
    pub async fn insert_idle_drv_for_requester(
        &self,
        drv: Drv,
        rid: RequesterId,
        target: Option<String>,
    ) {
        // Only process real derivations (32-char base32 hashes)
        if drv.hash.len() != 32 {
            return;
        }

        // First insert into dep tree (this queries nix and populates deps)
        {
            let mut state = self.write().await;
            state.already_built_drvs.remove(&drv);

            // Insert into dep_tree (queries Nix for dependencies)
            state.dep_tree.insert(drv.clone()).await;

            // Create BuildTarget with transitive closure
            if let Some(target_ref) = target {
                let target_id =
                    state.create_target(target_ref.clone(), drv.clone(), rid);
                tracing::info!(
                    "Created target {:?} for '{}' (drv: {})",
                    target_id,
                    target_ref,
                    drv.name
                );

                // Update target status after creation
                state.update_target_status(target_id);
            }

            // Check which drvs already have their required outputs in the store
            let mut built_drvs: HashSet<Drv> = HashSet::new();
            let mut checked_count = 0;
            let mut empty_paths_count = 0;

            for (d, node) in &state.dep_tree.nodes {
                // Check if all required output paths exist
                if !node.required_output_paths.is_empty() {
                    checked_count += 1;
                    let all_exist = node
                        .required_output_paths
                        .iter()
                        .all(|path| std::path::Path::new(path).exists());
                    if all_exist {
                        built_drvs.insert(d.clone());
                    }
                } else {
                    empty_paths_count += 1;
                }
            }

            tracing::error!(
                "output check: {} nodes total, {} checked, {} with empty \
                 paths, {} built",
                state.dep_tree.nodes.len(),
                checked_count,
                empty_paths_count,
                built_drvs.len()
            );

            if !built_drvs.is_empty() {
                for d in &built_drvs {
                    tracing::error!("  BUILT: {}", d.name);
                }
                state.already_built_drvs.extend(built_drvs);
            }

            tracing::error!(
                "already_built_drvs now has {} entries",
                state.already_built_drvs.len()
            );
        }
    }

    // TODO may want to batch eventually
    // or lock more coarsely
    // or maintain a set of diffs and then "merge"
    // that every few
    // seconds
    // but, this is fine for now
    pub async fn replace_build_job(
        &self,
        new_job @ BuildJob { jid: id, .. }: BuildJob,
    ) {
        let drv = new_job.drv.clone();

        let mut state = self.write().await;

        // Remove from already_built set if it was previously in there
        // (this is a new job for this drv)
        state.already_built_drvs.remove(&drv);

        match state.jid_to_job.entry(id) {
            Entry::Occupied(mut occupied_entry) => {
                occupied_entry.insert(new_job);
            }
            Entry::Vacant(vacant_entry) => {
                vacant_entry.insert(new_job);
            }
        }
        match state.drv_to_jobs.entry(drv.clone()) {
            Entry::Occupied(mut occupied_entry) => {
                occupied_entry.get_mut().insert(id);
            }
            Entry::Vacant(vacant_entry) => {
                let mut res = HashSet::new();
                res.insert(id);
                vacant_entry.insert(res);

                // Only query Nix for real derivations (32-char base32 hashes)
                // Synthetic hashes (16-char hex from activity IDs) won't exist
                if drv.hash.len() == 32 {
                    state.dep_tree.insert(drv.clone()).await;
                }
            }
        }
    }

    pub async fn mutate_build_job<F>(&self, id: JobId, mutator: F)
    where
        F: FnOnce(&mut BuildJob),
    {
        let mut state = self.write().await;
        match state.jid_to_job.entry(id) {
            Entry::Occupied(mut occupied_entry) => {
                let job = occupied_entry.get_mut();
                mutator(job);
            }
            Entry::Vacant(_vacant_entry) => {}
        }
    }

    /// Mark incomplete jobs for a given requester as cancelled or already built
    /// Called when a connection is terminated
    pub async fn cleanup_requester(&self, rid: RequesterId) {
        let mut state = self.write().await;

        // Find all targets for this requester
        let target_ids: Vec<BuildTargetId> = state
            .targets
            .values()
            .filter(|t| t.requester_id == rid)
            .map(|t| t.id)
            .collect();

        if target_ids.is_empty() {
            tracing::info!("cleanup requester {:?}: no targets found", rid);
            return;
        }

        tracing::info!(
            "cleanup requester {:?}: cleaning up {} targets",
            rid,
            target_ids.len()
        );

        // Check if any jobs were created for this requester
        let requester_had_jobs =
            state.jid_to_job.values().any(|j| j.rid == rid);

        // Check if any jobs are still active (not completed)
        let has_active_jobs = state.jid_to_job.values().any(|j| {
            j.rid == rid
                && !matches!(
                    j.status,
                    JobStatus::Cancelled
                        | JobStatus::CompletedBuild
                        | JobStatus::CompletedDownload
                        | JobStatus::CompletedSubstitute
                        | JobStatus::CompletedCopy
                        | JobStatus::CompletedQuery
                        | JobStatus::CompletedEvaluation
                        | JobStatus::CompletedSourceCopy
                        | JobStatus::AlreadyBuilt
                )
        });

        // Mark any active/pending jobs as cancelled
        let mut cancelled_count = 0;

        for (_jid, job) in state.jid_to_job.iter_mut() {
            if job.rid == rid
                && !matches!(
                    job.status,
                    JobStatus::Cancelled
                        | JobStatus::CompletedBuild
                        | JobStatus::CompletedDownload
                        | JobStatus::CompletedSubstitute
                        | JobStatus::CompletedCopy
                        | JobStatus::CompletedQuery
                        | JobStatus::CompletedEvaluation
                        | JobStatus::CompletedSourceCopy
                        | JobStatus::AlreadyBuilt
                )
            {
                job.status = JobStatus::Cancelled;
                job.stop_time_ns =
                    Some(START_INSTANT.elapsed().as_nanos() as u64);
                cancelled_count += 1;
            }
        }

        // Update status for each target
        for target_id in &target_ids {
            // Clone data we need before mutating state
            let (target_ref, transitive_closure) = if let Some(target) =
                state.targets.get(target_id)
            {
                (target.reference.clone(), target.transitive_closure.clone())
            } else {
                continue;
            };

            // Check if all drvs in target's closure are already built
            let all_already_built = transitive_closure
                .iter()
                .all(|drv| state.already_built_drvs.contains(drv));

            if !requester_had_jobs || !has_active_jobs || all_already_built {
                // Build completed from cache - mark all drvs as already built
                tracing::info!(
                    "Target '{}' completed from cache ({} drvs)",
                    target_ref,
                    transitive_closure.len()
                );

                for drv in &transitive_closure {
                    if !state.already_built_drvs.contains(drv) {
                        state.already_built_drvs.insert(drv.clone());
                    }
                }
            } else {
                // Target was cancelled
                tracing::info!(
                    "Target '{}' cancelled ({} drvs)",
                    target_ref,
                    transitive_closure.len()
                );
            }

            // Update the target's status based on jobs
            state.update_target_status(*target_id);
        }

        tracing::info!(
            "cleanup requester {:?}: {} jobs cancelled",
            rid,
            cancelled_count
        );
    }
}

pub struct SocketGuard(PathBuf);
impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn setup_unix_socket(
    path: &Path,
    mode: u32,
) -> io::Result<(UnixListener, SocketGuard)> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }

    if let Ok(md) = fs::symlink_metadata(path) {
        if md.file_type().is_socket() {
            fs::remove_file(path)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} exists and is not a socket", path.display()),
            ));
        }
    }

    let listener = UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;

    Ok((listener, SocketGuard(path.to_path_buf())))
}

pub async fn handle_daemon_info(
    socket_path: PathBuf,
    mode: u32,
    shutdown: Shutdown,
    info_builds: watch::Sender<JobsStateInner>,
) {
    // Call the socket function **once**
    let (listener, _guard) = setup_unix_socket(&socket_path, mode)
        .expect("setup_unix_socket failed");

    let mut ticker = interval(Duration::from_millis(100));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let cur_state: JobsState = Default::default();
    let mut next_rid: RequesterId = 0.into();
    let mut ticks_without_connection: u32 = 0;
    let mut warned_no_connection = false;
    let mut last_state_send = std::time::Instant::now();
    let (s, r) = unbounded_channel();
    let cur_state__ = cur_state.clone();
    let shutdown__ = shutdown.clone();
    spawn_named("line-handler", async move {
        handle_lines(r, cur_state__, shutdown__).await
    });
    loop {
        let accept_fut = listener.accept();
        tokio::pin!(accept_fut);
        let shutdown_fut = shutdown.wait();
        tokio::pin!(shutdown_fut);

        loop {
            tokio::select! {
                _done = &mut shutdown_fut => {
                    return;
                }
                //biased;
                res = &mut accept_fut => {
                    // it seems that a socket is opened per requester
                    match res {
                        Ok((stream, _addr)) =>
                            {
                                let rid = next_rid;
                                error!("ACCEPTED SOCKET {next_rid:?}");
                                next_rid = (*next_rid + 1).into();
                                ticks_without_connection = 0;
                                warned_no_connection = false;
                                let shutdown_ = shutdown.clone();
                                let cur_state_ = cur_state.clone();
                                let s_ = s.clone();
                                let fut = async move {
                                    if let Err(e)
                                        = read_stream(
                                            Box::new(stream),
                                            cur_state_,
                                            shutdown_,
                                            rid,
                                            s_
                                            ).await {
                                        error!("client connection error: {e}");
                                    }
                                }.boxed();

                                spawn_named(&format!("client-socket-handler-{rid:?}"), fut);

                            }
                        Err(e) => error!("accept error: {e}"),
                    }
                    break; // Break inner loop to create new accept future
                }
                _ = ticker.tick() => {

                    // Send state update every second - use try_read to never block accept
                    if last_state_send.elapsed() >= Duration::from_secs(1) {
                        last_state_send = std::time::Instant::now();

                        // CRITICAL: Use try_read() to never block socket accepts
                        if let Ok(tmp_state) = cur_state.try_read() {
                            error!("SENDING A STATE UPDATE!!!");
                            if info_builds.send(tmp_state.clone()).is_err() {
                                error!("no active receivers for info_builds");
                            }
                        } else {
                            // State is locked - skip this update to keep accept fast
                        }

                        // Warn if no Nix connections have been received
                        ticks_without_connection =
                            ticks_without_connection.saturating_add(1);
                        if !warned_no_connection
                            && ticks_without_connection >= 5 {
                            warned_no_connection = true;
                            error!("No Nix log connections received after {} seconds", ticks_without_connection);
                            error!("Make sure your nix.conf has: extra-experimental-features = nix-command");
                            error!("And: json-log-path = {}", socket_path.display());
                            error!("Then run your nix build with: nix build --log-format internal-json -vvv ...");
                        }
                    }
                }
            }
        }
    }
    // TODO remove socket path
}

#[derive(
    Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Ord, PartialOrd,
)]
pub struct BuildJob {
    pub jid: JobId,
    pub rid: RequesterId,
    pub drv: Drv,
    pub status: JobStatus,
    pub start_time_ns: u64,
    pub stop_time_ns: Option<u64>,
}

#[repr(transparent)]
#[derive(
    Copy,
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    Default,
)]
#[serde(transparent)]
pub struct RequesterId(pub u64);

impl From<u64> for RequesterId {
    fn from(value: u64) -> Self {
        RequesterId(value)
    }
}

impl Deref for RequesterId {
    type Target = u64;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl BuildJob {
    pub fn new(jid: JobId, rid: RequesterId, drv: Drv) -> Self {
        BuildJob {
            rid,
            jid,
            drv,
            status: JobStatus::Starting,
            start_time_ns: START_INSTANT.elapsed().as_nanos() as u64,
            stop_time_ns: None,
        }
    }

    pub fn runtime(&self) -> u64 {
        let end_ns = self
            .stop_time_ns
            .unwrap_or_else(|| START_INSTANT.elapsed().as_nanos() as u64);
        end_ns.saturating_sub(self.start_time_ns)
    }
}

pub fn parse_to_str<'a>(
    fields: Option<&Vec<Field<'a>>>,
    idx: usize,
) -> Option<String> {
    fields.and_then(|v| v.get(idx)).and_then(|f| match f {
        Field::String(cow) => Some(cow.as_ref().to_str_lossy().into_owned()),
        _ => None,
    })
}

#[derive(
    Clone,
    Debug,
    Serialize,
    Deserialize,
    Ord,
    PartialOrd,
    Eq,
    PartialEq,
    Default,
)]
pub enum JobStatus {
    /// Build is in a specific phase (e.g., "unpackPhase", "buildPhase")
    BuildPhaseType(String /* phase name */),
    /// Build activity just started
    Starting,
    /// Build completed successfully
    CompletedBuild,
    /// Querying a cache for path info
    Querying(String /* cache name */),
    /// Query completed (found or not found)
    CompletedQuery,
    /// Downloading a file/narinfo
    Downloading {
        url: String,
        done_bytes: u64,
        total_bytes: u64,
    },
    /// Download completed
    CompletedDownload,
    /// Substituting (copying from cache to local store)
    Substituting {
        store_path: String,
        cache_name: String,
    },
    /// Substitution completed
    CompletedSubstitute,
    /// Copying a path (between stores)
    Copying {
        path: String,
        done_bytes: u64,
        total_bytes: u64,
    },
    /// Copy completed
    CompletedCopy,
    /// Path already exists in local store (cache hit)
    AlreadyBuilt,
    /// Waiting for a build lock
    WaitingForLock,
    /// Running post-build hook
    PostBuildHook,
    /// Fetching a flake tree
    FetchingTree(String /* url */),
    /// Evaluating Nix expressions
    Evaluating,
    /// Evaluation completed
    CompletedEvaluation,
    /// Copying source files to store
    CopyingSource,
    /// Source copy completed
    CompletedSourceCopy,
    /// Queued to be built (we know it will be built, but hasn't started)
    Queued,
    #[default]
    NotEnoughInfo,
    Cancelled,
}

impl JobStatus {
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            JobStatus::BuildPhaseType(_)
                | JobStatus::Starting
                | JobStatus::Querying(_)
                | JobStatus::Downloading { .. }
                | JobStatus::Substituting { .. }
                | JobStatus::Copying { .. }
                | JobStatus::WaitingForLock
                | JobStatus::PostBuildHook
                | JobStatus::FetchingTree(_)
                | JobStatus::Evaluating
                | JobStatus::CopyingSource
        )
    }

    /// Returns true if this job is queued/pending (will be built but hasn't
    /// started)
    pub fn is_pending(&self) -> bool {
        matches!(self, JobStatus::Queued)
    }

    /// Returns true if this job is active OR pending (i.e., not completed and
    /// not unknown)
    pub fn is_in_progress(&self) -> bool {
        self.is_active() || self.is_pending()
    }

    pub fn is_completed(&self) -> bool {
        matches!(
            self,
            JobStatus::CompletedBuild
                | JobStatus::CompletedQuery
                | JobStatus::CompletedDownload
                | JobStatus::CompletedSubstitute
                | JobStatus::CompletedCopy
                | JobStatus::AlreadyBuilt
                | JobStatus::CompletedEvaluation
                | JobStatus::CompletedSourceCopy
        )
    }

    pub fn mark_complete(&mut self) -> Self {
        match self {
            JobStatus::BuildPhaseType(_) | JobStatus::Starting => {
                JobStatus::CompletedBuild
            }
            JobStatus::Querying(_) => JobStatus::CompletedQuery,
            JobStatus::Downloading { .. } => JobStatus::CompletedDownload,
            JobStatus::Substituting { .. } => JobStatus::CompletedSubstitute,
            JobStatus::Copying { .. } => JobStatus::CompletedCopy,
            JobStatus::FetchingTree(_) => JobStatus::CompletedDownload,
            JobStatus::PostBuildHook => JobStatus::CompletedBuild,
            JobStatus::Evaluating => JobStatus::CompletedEvaluation,
            JobStatus::CopyingSource => JobStatus::CompletedSourceCopy,
            _ => self.clone(),
        }
    }
}

impl Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobStatus::BuildPhaseType(s) => {
                write!(f, "Building: {s}")
            }
            JobStatus::Starting => write!(f, "Starting"),
            JobStatus::CompletedBuild => write!(f, "Built"),
            JobStatus::Querying(s) => {
                write!(f, "Querying {s}")
            }
            JobStatus::CompletedQuery => write!(f, "Query done"),
            JobStatus::Downloading {
                url,
                done_bytes,
                total_bytes,
            } => {
                if *total_bytes > 0 {
                    let pct =
                        (*done_bytes as f64 / *total_bytes as f64) * 100.0;
                    write!(
                        f,
                        "Downloading {:.1}% ({}/{})",
                        pct,
                        format_bytes(*done_bytes),
                        format_bytes(*total_bytes)
                    )
                } else {
                    write!(
                        f,
                        "Downloading {} ({})",
                        url,
                        format_bytes(*done_bytes)
                    )
                }
            }
            JobStatus::CompletedDownload => write!(f, "Downloaded"),
            JobStatus::Substituting {
                store_path: _,
                cache_name,
            } => {
                write!(f, "Substituting from {cache_name}")
            }
            JobStatus::CompletedSubstitute => write!(f, "Substituted"),
            JobStatus::Copying {
                path: _,
                done_bytes,
                total_bytes,
            } => {
                if *total_bytes > 0 {
                    let pct =
                        (*done_bytes as f64 / *total_bytes as f64) * 100.0;
                    write!(
                        f,
                        "Copying {:.1}% ({}/{})",
                        pct,
                        format_bytes(*done_bytes),
                        format_bytes(*total_bytes)
                    )
                } else {
                    write!(f, "Copying ({})", format_bytes(*done_bytes))
                }
            }
            JobStatus::CompletedCopy => write!(f, "Copied"),
            JobStatus::AlreadyBuilt => write!(f, "Already built"),
            JobStatus::WaitingForLock => write!(f, "Waiting for lock"),
            JobStatus::PostBuildHook => write!(f, "Post-build hook"),
            JobStatus::FetchingTree(url) => write!(f, "Fetching {url}"),
            JobStatus::Evaluating => write!(f, "Evaluating"),
            JobStatus::CompletedEvaluation => write!(f, "Evaluated"),
            JobStatus::CopyingSource => write!(f, "Copying source"),
            JobStatus::CompletedSourceCopy => write!(f, "Source copied"),
            JobStatus::Queued => write!(f, "Queued"),
            JobStatus::NotEnoughInfo => write!(f, "Unknown"),
            JobStatus::Cancelled => write!(f, "Cancelled"),
        }
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1}GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1}MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{}B", bytes)
    }
}

pub fn format_secs(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    let seconds = secs % 60;

    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    parts.push(format!("{seconds}s"));

    parts.join(" ")
}

pub fn format_duration(dur_ns: u64) -> String {
    let secs = Duration::from_nanos(dur_ns).as_secs();
    format_secs(secs)
}

async fn handle_line(line: String, state: JobsState, rid: RequesterId) {
    match LogMessage::from_json_str(&line) {
        Ok(msg) => {
            //println!("{:?}", msg);
            //let msg_ = msg.clone();
            match msg {
                LogMessage::Start {
                    fields,
                    id,
                    //level,
                    //parent,
                    text,
                    r#type,
                    ..
                } => match r#type {
                    ActivityType::Unknown => {
                        // Type 0 is used for various activities, check text
                        let text_str = text.to_string();
                        if text_str.starts_with("evaluating derivation") {
                            // Extract the target from "evaluating derivation
                            // 'target'..."
                            // Note: Nix sometimes adds "..." at the end
                            if let Some(target) = text_str
                                .strip_prefix("evaluating derivation '")
                                .and_then(|s| {
                                    s.strip_suffix("'...")
                                        .or_else(|| s.strip_suffix("'"))
                                })
                            {
                                // Evaluate the flake reference to get the .drv
                                // path
                                // This gives us the top-level derivation for
                                // the dependency tree
                                let state_clone = state.clone();
                                let target_owned = target.to_string();
                                tracing::error!(
                                    "🎯 Detected target: '{}', spawning eval \
                                     task",
                                    target_owned
                                );
                                spawn_named(
                                    &format!("evaluating {target_owned:?}"),
                                    async move {
                                        tracing::error!(
                                            "📊 Starting eval for target: '{}'",
                                            target_owned
                                        );
                                        match eval_flake_to_drv(&target_owned)
                                            .await
                                        {
                                            Some(drv) => {
                                                tracing::error!(
                                                    "✅ Eval SUCCESS for '{}' \
                                                     -> drv: {}",
                                                    target_owned,
                                                    drv.name
                                                );
                                                tracing::error!(
                                                    "📝 About to call insert_idle_drv_for_requester for '{}'",
                                                    target_owned
                                                );
                                                state_clone
                                                    .insert_idle_drv_for_requester(
                                                        drv,
                                                        rid,
                                                        Some(target_owned.clone()),
                                                    )
                                                    .await;
                                                tracing::error!(
                                                    "✅ insert_idle_drv_for_requester completed for '{}'",
                                                    target_owned
                                                );
                                            }
                                            None => {
                                                tracing::error!(
                                                    "❌ Eval FAILED for \
                                                     target: '{}'",
                                                    target_owned
                                                );
                                            }
                                        }
                                    },
                                );
                            }

                            // Evaluation activity - create a job to track it
                            let drv = Drv {
                                hash: format!("{:016x}", id),
                                name: "evaluation".to_string(),
                            };
                            let new_job = BuildJob {
                                status: JobStatus::Evaluating,
                                ..BuildJob::new(id.into(), rid, drv)
                            };
                            state.replace_build_job(new_job).await;
                        } else if text_str.starts_with("copying") {
                            // Copying source files to store
                            // Extract filename from text like "copying '...' to
                            // the store"
                            let name = text_str
                                .strip_prefix("copying '")
                                .and_then(|s| s.split('\'').next())
                                .map(|s| {
                                    // Get just the filename
                                    s.rsplit('/')
                                        .next()
                                        .unwrap_or(s)
                                        .to_string()
                                })
                                .unwrap_or_else(|| "source".to_string());

                            let drv = Drv {
                                hash: format!("{:016x}", id),
                                name,
                            };
                            let new_job = BuildJob {
                                status: JobStatus::CopyingSource,
                                ..BuildJob::new(id.into(), rid, drv)
                            };
                            state.replace_build_job(new_job).await;
                        }
                    }
                    ActivityType::CopyPath => {
                        // fields: [from_path, to_path] or just [path]
                        let fr = fields.as_ref();
                        if let Some(path_str) = parse_to_str(fr, 0) {
                            let maybe_drv: Option<Drv> =
                                match parse_store_path(&path_str) {
                                    Either::Left(d) => Some(d),
                                    Either::Right(so) => so.get_drv().await,
                                };
                            if let Some(drv) = maybe_drv {
                                let new_job = BuildJob {
                                    status: JobStatus::Copying {
                                        path: path_str,
                                        done_bytes: 0,
                                        total_bytes: 0,
                                    },
                                    ..BuildJob::new(id.into(), rid, drv)
                                };
                                state.replace_build_job(new_job).await;
                            }
                        }
                    }
                    ActivityType::FileTransfer => {
                        // fields: [url]
                        let fr = fields.as_ref();
                        if let Some(url) = parse_to_str(fr, 0) {
                            // Try to extract store path from URL
                            // e.g., https://cache.nixos.org/abc123.narinfo
                            if let Some(hash) = extract_hash_from_url(&url) {
                                let drv = Drv {
                                    hash,
                                    name: "download".to_string(),
                                };
                                let new_job = BuildJob {
                                    status: JobStatus::Downloading {
                                        url: url.clone(),
                                        done_bytes: 0,
                                        total_bytes: 0,
                                    },
                                    ..BuildJob::new(id.into(), rid, drv)
                                };
                                state.replace_build_job(new_job).await;
                            }
                        }
                    }
                    ActivityType::Realise => (),
                    ActivityType::CopyPaths => (),
                    ActivityType::Builds => {}
                    ActivityType::Build => {
                        // nix store paths are supposed to be valid utf8
                        if let Some(drv_str) = parse_to_str(fields.as_ref(), 0)
                        {
                            let maybe_drv: Option<Drv> =
                                match parse_store_path(&drv_str) {
                                    Either::Left(d) => Some(d),
                                    Either::Right(so) => so.get_drv().await,
                                };
                            if let Some(drv) = maybe_drv {
                                let new_job =
                                    BuildJob::new(id.into(), rid, drv);
                                state.replace_build_job(new_job).await;
                            }
                        }
                    }
                    ActivityType::OptimiseStore => {}
                    ActivityType::VerifyPaths => (),
                    ActivityType::Substitute => {
                        // fields: [store_path, cache_uri]
                        let fr = fields.as_ref();
                        if let (Some(path_str), Some(cache)) =
                            (parse_to_str(fr, 0), parse_to_str(fr, 1))
                        {
                            let maybe_drv: Option<Drv> =
                                match parse_store_path(&path_str) {
                                    Either::Left(d) => Some(d),
                                    Either::Right(so) => so.get_drv().await,
                                };
                            if let Some(drv) = maybe_drv {
                                let new_job = BuildJob {
                                    status: JobStatus::Substituting {
                                        store_path: path_str,
                                        cache_name: cache,
                                    },
                                    ..BuildJob::new(id.into(), rid, drv)
                                };
                                state.replace_build_job(new_job).await;
                            }
                        }
                    }
                    ActivityType::QueryPathInfo => {
                        let fr = fields.as_ref();
                        if let (Some(drv_str), Some(cache)) =
                            (parse_to_str(fr, 0), parse_to_str(fr, 1))
                        {
                            let maybe_drv: Option<Drv> =
                                match parse_store_path(&drv_str) {
                                    Either::Left(d) => Some(d),
                                    Either::Right(so) => so.get_drv().await,
                                };
                            if let Some(drv) = maybe_drv {
                                let new_job = BuildJob {
                                    status: JobStatus::Querying(cache),
                                    ..BuildJob::new(id.into(), rid, drv)
                                };
                                state.replace_build_job(new_job).await;
                            }
                        }
                    }
                    ActivityType::PostBuildHook => {
                        // Try to get the drv from parent or text
                        if let Some(drv_str) = parse_to_str(fields.as_ref(), 0)
                        {
                            let maybe_drv: Option<Drv> =
                                match parse_store_path(&drv_str) {
                                    Either::Left(d) => Some(d),
                                    Either::Right(so) => so.get_drv().await,
                                };
                            if let Some(drv) = maybe_drv {
                                let new_job = BuildJob {
                                    status: JobStatus::PostBuildHook,
                                    ..BuildJob::new(id.into(), rid, drv)
                                };
                                state.replace_build_job(new_job).await;
                            }
                        }
                    }
                    ActivityType::BuildWaiting => {
                        // Build is waiting for a lock
                        if let Some(drv_str) = parse_to_str(fields.as_ref(), 0)
                        {
                            let maybe_drv: Option<Drv> =
                                match parse_store_path(&drv_str) {
                                    Either::Left(d) => Some(d),
                                    Either::Right(so) => so.get_drv().await,
                                };
                            if let Some(drv) = maybe_drv {
                                let new_job = BuildJob {
                                    status: JobStatus::WaitingForLock,
                                    ..BuildJob::new(id.into(), rid, drv)
                                };
                                state.replace_build_job(new_job).await;
                            }
                        }
                    }
                    ActivityType::FetchTree => {
                        // fields: [url] or similar
                        let url = parse_to_str(fields.as_ref(), 0)
                            .unwrap_or_else(|| text.to_string());
                        // Create a pseudo-drv for the fetch
                        let drv = Drv {
                            hash: format!("{:016x}", id),
                            name: "fetch-tree".to_string(),
                        };
                        let new_job = BuildJob {
                            status: JobStatus::FetchingTree(url),
                            ..BuildJob::new(id.into(), rid, drv)
                        };
                        state.replace_build_job(new_job).await;
                    }
                },
                LogMessage::Stop { id } => {
                    state.stop_build_job(id.into()).await;
                }
                LogMessage::Result { fields, id, r#type } => match r#type {
                    json_parsing_nix::ResultType::FileLinked => (),
                    json_parsing_nix::ResultType::BuildLogLine => (),
                    json_parsing_nix::ResultType::UntrustedPath => (),
                    json_parsing_nix::ResultType::CorruptedPath => (),
                    json_parsing_nix::ResultType::SetPhase => {
                        if let Some(phase_name) =
                            fields.first().and_then(|f| match f {
                                Field::String(cow) => Some(
                                    cow.as_ref().to_str_lossy().into_owned(),
                                ),
                                _ => None,
                            })
                        {
                            state
                                .mutate_build_job(id.into(), move |job| {
                                    job.status =
                                        JobStatus::BuildPhaseType(phase_name);
                                })
                                .await;
                        }
                    }
                    json_parsing_nix::ResultType::Progress => {
                        // Progress fields: [done, expected, running, failed]
                        // These are typically byte counts for downloads/copies
                        if fields.len() >= 2 {
                            let done = fields[0].as_int().unwrap_or(0);
                            let expected = fields[1].as_int().unwrap_or(0);

                            state
                                .mutate_build_job(id.into(), move |job| {
                                    // Update progress based on current status
                                    // type
                                    match &mut job.status {
                                        JobStatus::Downloading {
                                            done_bytes,
                                            total_bytes,
                                            ..
                                        } => {
                                            *done_bytes = done;
                                            *total_bytes = expected;
                                        }
                                        JobStatus::Copying {
                                            done_bytes,
                                            total_bytes,
                                            ..
                                        } => {
                                            *done_bytes = done;
                                            *total_bytes = expected;
                                        }
                                        JobStatus::Substituting { .. } => {
                                            // Could track substitution progress
                                            // if needed
                                        }
                                        _ => {
                                            // For other types, we could track
                                            // generic progress
                                        }
                                    }
                                })
                                .await;
                        }
                    }
                    json_parsing_nix::ResultType::SetExpected => (),
                    json_parsing_nix::ResultType::PostBuildLogLine => (),
                    json_parsing_nix::ResultType::FetchStatus => {
                        // FetchStatus can provide URL status updates
                        if let Some(status_str) =
                            fields.first().and_then(|f| match f {
                                Field::String(cow) => Some(
                                    cow.as_ref().to_str_lossy().into_owned(),
                                ),
                                _ => None,
                            })
                        {
                            state
                                .mutate_build_job(id.into(), move |job| {
                                    if let JobStatus::Downloading {
                                        url, ..
                                    } = &job.status
                                    {
                                        // Could update with status info
                                        let _ = (url, status_str);
                                    }
                                })
                                .await;
                        }
                    }
                },
                LogMessage::Msg { level, msg } => {
                    if level == VerbosityLevel::Info {
                        match parse_msg_info_sync(&msg) {
                            MsgParseResult::PlannedBuilds(drvs) => {
                                // We have all the planned builds in one message
                                // The LAST one is the top-level target
                                // Insert only the last one - it will pull in
                                // all deps via --recursive
                                if let Some(top_level) = drvs.last().cloned() {
                                    tracing::info!(
                                        "top-level target: {} ({} total \
                                         planned builds)",
                                        top_level.name,
                                        drvs.len()
                                    );
                                    state.insert_idle_drv(top_level).await;
                                }
                            }
                            MsgParseResult::Other => {
                                tracing::trace!(
                                    "rid: {rid:?}, unhandled info msg: {msg}"
                                );
                            }
                        }
                    } else {
                        tracing::trace!(
                            "verbositylvl {level:?} received msg {msg}"
                        );
                    }
                }
                LogMessage::SetPhase { .. } => {}
            }
        }
        Err(e) => {
            eprintln!("JSON error: {e}\n\tline was: {line}")
        }
    }
}

/// Result of parsing a Msg for planned builds
enum MsgParseResult {
    /// All planned derivations from a "will be built" message
    /// The last one is the top-level target
    PlannedBuilds(Vec<Drv>),
    /// Not a planned build message
    Other,
}

fn parse_msg_info_sync(msg: &str) -> MsgParseResult {
    // Check if this is a "will be built" message
    let re_header = Regex::new(r"(?:these\s+\d+\s+derivations?\s+will\s+be\s+built:|this derivation will be built:)")
        .unwrap();

    if !re_header.is_match(msg) {
        return MsgParseResult::Other;
    }

    // Extract all .drv paths from the message
    let re_drv =
        Regex::new(r"/nix/store/([a-z0-9]{32})-([^\s]+)\.drv").unwrap();

    let drvs: Vec<Drv> = re_drv
        .captures_iter(msg)
        .map(|caps| {
            let hash = caps[1].to_string();
            let name = caps[2].to_string();
            Drv { name, hash }
        })
        .collect();

    if drvs.is_empty() {
        return MsgParseResult::Other;
    }

    tracing::debug!(
        "parsed {} planned builds, last (top-level): {}",
        drvs.len(),
        drvs.last().map(|d| d.name.as_str()).unwrap_or("?")
    );

    MsgParseResult::PlannedBuilds(drvs)
}

pub fn parse_store_path(path: &str) -> Either<Drv, StoreOutput> {
    let s = path.strip_prefix("/nix/store/").unwrap_or(path);

    let (hash, mut name) = match s.split_once('-') {
        Some((h, n)) => (h.to_string(), n.to_string()),
        None => (s.to_string(), String::new()),
    };

    if name.ends_with(".drv") {
        name.truncate(name.len() - 4);
        Either::Left(Drv { hash, name })
    } else {
        Either::Right(StoreOutput { hash, name })
    }
}

/// Extract a Nix store hash from a URL like:
/// - https://cache.nixos.org/abc123def456.narinfo
/// - https://cache.nixos.org/nar/abc123def456.nar.xz
fn extract_hash_from_url(url: &str) -> Option<String> {
    // Try to find a 32-character base32 hash in the URL
    let re = Regex::new(r"/([a-z0-9]{32})(?:\.narinfo|\.nar)").ok()?;
    re.captures(url)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
}

async fn read_stream(
    stream: Box<UnixStream>,
    _state: JobsState,
    //info_builds: watch::Sender<HashMap<u64, BuildJob>>,
    shutdown: Shutdown,
    rid: RequesterId,
    chan: UnboundedSender<(RequesterId, Either<String, ()>)>,
) -> io::Result<()> {
    let reader = BufReader::new(stream);
    let mut lines = reader.lines();

    let shutdown_fut = shutdown.wait().boxed();
    tokio::pin!(shutdown_fut);

    loop {
        tokio::select! {
            _ = &mut shutdown_fut => {
                break;

            }
            result = lines.next_line() => {
                match result {
                    Ok(Some(line)) => {
                        if chan.send((rid, Either::Left(line))).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {
                        let _ = chan.send((rid, Either::Right(())));
                        break;
                    }
                    Err(e) => {
                        error!("error reading from unix stream for {rid:?}: {e}");

                        let _ = chan.send((rid, Either::Right(())));
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}
pub async fn handle_lines(
    mut chan: UnboundedReceiver<(RequesterId, Either<String, ()>)>,
    state: JobsState,
    is_shutdown: Shutdown,
) {
    let shutdown_fut = is_shutdown.wait();
    tokio::pin!(shutdown_fut);
    loop {
        tokio::select!(
            _ = &mut shutdown_fut => {
                error!("handle_lines shutting down");
                return;
            }
            res = chan.recv() => {
                match res {
                    Some((rid, msg)) => {
                        match msg {
                            Either::Left(line) => {
                                handle_line(line, state.clone(), rid).await;
                            }
                            // Stream closed or error on that UnixStream
                            Either::Right(()) => {
                                error!("stopped being able to read socket on {rid:?}");
                                // Clean up jobs for this requester on connection close
                                state.cleanup_requester(rid).await;
                            }
                        }

                    },
                    None => {
                        error!("handle_lines channel closed");
                        return;
                    },
                }
            }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_msg_info_single_build() {
        let msg = "this derivation will be built:\n  \
                   /nix/store/31xvpflz5asihsmyl088cgxyxwflzrz3-coreutils-9.7.\
                   drv";
        let result = parse_msg_info_sync(msg);
        match result {
            MsgParseResult::PlannedBuilds(drvs) => {
                assert_eq!(drvs.len(), 1);
                assert_eq!(drvs[0].name, "coreutils-9.7");
            }
            _ => panic!("expected PlannedBuilds"),
        }
    }

    #[test]
    fn test_parse_msg_info_multiple_builds() {
        let msg = "these 3 derivations will be built:\n  \
                   /nix/store/abc123def456abc123def456abc12345-bison-3.8.2.\
                   drv\n  /nix/store/def456abc123def456abc123def45678-bash-5.\
                   3.drv\n  /nix/store/789012abc123def456abc123def45678-bat-0.\
                   24.0.drv";
        let result = parse_msg_info_sync(msg);
        match result {
            MsgParseResult::PlannedBuilds(drvs) => {
                assert_eq!(drvs.len(), 3);
                assert_eq!(drvs[0].name, "bison-3.8.2");
                assert_eq!(drvs[1].name, "bash-5.3");
                assert_eq!(drvs[2].name, "bat-0.24.0"); // last is top-level
            }
            _ => panic!("expected PlannedBuilds"),
        }
    }

    #[test]
    fn test_parse_msg_info_not_build_message() {
        let msg = "some other message";
        let result = parse_msg_info_sync(msg);
        assert!(matches!(result, MsgParseResult::Other));
    }
}
