use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    fmt::Display,
    fs, io,
    ops::Deref,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
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
    sync::{RwLock, watch},
    time::{MissedTickBehavior, interval},
};
use tracing::error;

use crate::derivation_tree::{DrvRelations, START_INSTANT};

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

#[derive(Clone, Debug, Default)]
pub struct JobsStateInner {
    pub jid_to_job: HashMap<JobId, BuildJob>,
    pub drv_to_jobs: HashMap<Drv, HashSet<JobId>>,
    pub dep_tree: DrvRelations,
}

impl JobsStateInner {
    pub fn get_status(&self, drv: &Drv) -> JobStatus {
        if let Some(jobs) = self.drv_to_jobs.get(drv) {
            for job in jobs {
                if let Some(bj) = self.jid_to_job.get(job) {
                    return bj.status.clone();
                }
            }
        }
        JobStatus::NotEnoughInfo
    }

    pub fn make_tree_description(&self, drv: &Drv) -> String {
        let status = self.get_status(drv);
        format!("{} - {} - {}", drv.name.clone(), drv.hash.clone(), status)
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
    try_from = "crate::client_daemon_comms::DrvWire",
    into = "crate::client_daemon_comms::DrvWire"
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
        state.dep_tree.insert(drv.clone()).await;
        let drv_node = state.dep_tree.nodes.get(&drv).unwrap().clone();
        state.dep_tree.insert_node(drv_node);
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

                state.dep_tree.insert(drv.clone()).await;
                let drv_node = state.dep_tree.nodes.get(&drv).unwrap().clone();
                state.dep_tree.insert_node(drv_node);
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
    is_shutdown: Arc<AtomicBool>,
    info_builds: watch::Sender<JobsStateInner>,
) {
    // Call the socket function **once**
    let (listener, _guard) = setup_unix_socket(&socket_path, mode)
        .expect("setup_unix_socket failed");

    let mut ticker = interval(Duration::from_secs(1));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let cur_state: JobsState = Default::default();
    let mut next_rid: RequesterId = 0.into();
    loop {
        tokio::select! {
            res = listener.accept() => {
                // it seems that a socket is opened per requester
                match res {
                    Ok((stream, _addr))
                        if !is_shutdown.load(Ordering::Relaxed) =>
                        {
                            let rid = next_rid;
                            error!("ACCEPTED SOCKET {next_rid:?}");
                            next_rid = (*next_rid + 1).into();
                            let is_shutdown_ = is_shutdown.clone();
                            let cur_state_ = cur_state.clone();
                            let fut = async move {
                                if let Err(e)
                                    = read_stream(
                                        Box::new(stream),
                                        cur_state_,
                                        is_shutdown_,
                                        rid
                                        ).await {
                                    error!("client connection error: {e}");
                                }
                            }.boxed();

                            let _handle = tokio::task::Builder::new().name(&format!("client-socket-handler-{rid:?}")).spawn(fut);
                        }
                    Ok((_stream, _addr)) => {
                    }

                    Err(e) => error!("accept error: {e}"),
                }
            }
            _ = ticker.tick() => {
                    let tmp_state = cur_state.read().await;
                    if info_builds.send(tmp_state.clone()).is_err() {
                        error!("no active receivers for info_builds");
                    }
                }
        }

        if is_shutdown.load(Ordering::Relaxed) {
            break;
        }
    }
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
    BuildPhaseType(String /* phase name */),
    Starting,
    CompletedBuild,
    Querying(String /* cache name */),
    CompletedQuery,
    Downloading {
        cache_name: String,
        narinfo_name: String,
    },
    #[default]
    NotEnoughInfo,
    Cancelled,
}

impl JobStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, JobStatus::BuildPhaseType(_))
    }
    pub fn mark_complete(&mut self) -> Self {
        match self {
            JobStatus::BuildPhaseType(_) => JobStatus::CompletedBuild,
            JobStatus::Querying(_) => JobStatus::CompletedQuery,
            _ => JobStatus::NotEnoughInfo,
        }
    }
}

impl Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobStatus::BuildPhaseType(s) => {
                write!(f, "Building Derivation, Phase is: {s}")
            }
            JobStatus::Starting => write!(f, "Starting"),
            JobStatus::CompletedBuild => write!(f, "Completed"),
            JobStatus::Querying(s) => {
                write!(f, "Querying cache {s} for narinfo")
            }
            JobStatus::CompletedQuery => write!(f, "Completed Query"),
            JobStatus::NotEnoughInfo => {
                write!(f, "Not enough information to make an inference")
            }
            JobStatus::Downloading {
                cache_name,
                narinfo_name,
            } => {
                write!(f, "Downloading {narinfo_name} from {cache_name}")
            }
            JobStatus::Cancelled => {
                write!(f, "Job was cancelled")
            }
        }
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
                    //text,
                    r#type,
                    ..
                } => match r#type {
                    ActivityType::Unknown => (),
                    ActivityType::CopyPath => (),
                    ActivityType::FileTransfer => (),
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
                        } else {
                            // TODO proper logging using the usual crate
                            //eprintln!(
                            //    "Error on either getting the fields, or that \
                            //     the fields are not valid utf8. Msg in \
                            //     question: {}",
                            //    &msg_
                            //);
                        }
                    }
                    ActivityType::OptimiseStore => {}
                    ActivityType::VerifyPaths => (),
                    ActivityType::Substitute => (),
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
                    ActivityType::PostBuildHook => {}
                    ActivityType::BuildWaiting => (),
                    ActivityType::FetchTree => (),
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
                        // TODO separate out into get_one_arg function
                        if let Some(phase_name) = Some(fields)
                            .as_ref()
                            .and_then(|v| v.first())
                            .and_then(|f| match f {
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
                        } else {
                            //eprintln!(
                            //    "Error on either getting the fields, or that \
                            //     the fields are not valid utf8. Msg in \
                            //     question: {}",
                            //    msg_
                            //);
                        }
                    }
                    json_parsing_nix::ResultType::Progress => (),
                    json_parsing_nix::ResultType::SetExpected => (),
                    json_parsing_nix::ResultType::PostBuildLogLine => (),
                    json_parsing_nix::ResultType::FetchStatus => (),
                },
                LogMessage::Msg { level, msg } => {
                    if level == VerbosityLevel::Info {
                        if let Some(drv) = parse_msg_info(msg, rid).await {
                            state.insert_idle_drv(drv).await
                        }
                    } else {
                        error!("verbositylvl {level:?} received msg {msg}");
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

async fn parse_msg_info(
    msg: std::borrow::Cow<'_, str>,
    rid: RequesterId,
) -> Option<Drv> {
    let re = Regex::new(r"these\s+(\d+)\s+derivations?\s+will\s+be\s+built:")
        .unwrap();
    let re2 = Regex::new(r"^\s*/nix/store/([a-z0-9]{32})-(.+)\.drv$").unwrap();
    let re3 = Regex::new(r" this derivation will be built:").unwrap();
    let re4 = Regex::new(r"^\s*/nix/store/([a-z0-9]{32})-(.+)$").unwrap();

    if let Some(caps) = re.captures(&msg) {
        let count: u32 = caps[1].parse().unwrap();
        error!("rid: {rid:?}, recved {count} derivations");
        //println!("{} derivations", count);
    } else if let Some(_caps) = re3.captures(&msg) {
        let _count = 1;
    } else if let Some(caps) = re2.captures(&msg) {
        let hash = caps[1].to_string();
        let name = caps[2].to_string();
        // okay yeah this is repetitive/unnecessary
        let drv = format!("/nix/store/{hash}-{name}.drv");
        error!("rid: {rid:?}, building {drv}");
        return Some(Drv { name, hash });
    } else if let Some(caps) = re4.captures(&msg) {
        let hash = caps[1].to_string();
        let name = caps[2].to_string();
        let _drv = format!("/nix/store/{hash}-{name}");
        let store_output = StoreOutput { name, hash };
        return store_output.get_drv().await;

        //error!("rid: {rid}, building {drv}");
        //return Some(Drv { name, hash });
    } else {
        error!("rid: {rid:?}, could not parse {msg}");
    }
    None
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

async fn read_stream(
    stream: Box<UnixStream>,
    state: JobsState,
    //info_builds: watch::Sender<HashMap<u64, BuildJob>>,
    is_shutdown: Arc<AtomicBool>,
    rid: RequesterId,
) -> io::Result<()> {
    let reader = BufReader::new(stream);
    let mut lines = reader.lines();

    loop {
        if is_shutdown.load(Ordering::Relaxed) {
            return Ok(());
        }
        if let Ok(Some(line)) = lines.next_line().await {
            handle_line(line, state.clone(), rid).await;
        } else {
            error!("stopped being able to read socket on {rid:?}");
            return Ok(());
        }
    }
}
#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;

    #[tokio::test]
    async fn test_parse_msg_info_drv_line() {
        let msg = Cow::Borrowed(
            "  /nix/store/31xvpflz5asihsmyl088cgxyxwflzrz3-coreutils-9.7.drv",
        );
        parse_msg_info(msg, 0.into()).await;
    }

    #[tokio::test]
    async fn test_parse_msg_info_build_count() {
        let msg = Cow::Borrowed("these 93 derivations will be built:");
        parse_msg_info(msg, 0.into()).await;
    }
}
