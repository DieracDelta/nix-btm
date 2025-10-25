use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    fmt::Display,
    fs, io,
    ops::Deref,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use bstr::ByteSlice as _;
use json_parsing_nix::{ActivityType, Field, LogMessage};
use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    net::{UnixListener, UnixStream},
    sync::{RwLock, watch},
    time::{MissedTickBehavior, interval},
};

//#[derive(Debug)]
//pub struct LogLine {
//    pub actionUpdate: ActionUpdate,
//    pub fields: ActivityUpdateFields,
//    pub id: u64,
//    pub type_: ActivityTypeTag,
//    pub text: Option<String>,
//}
//
//#[derive(Debug, Deserialize)]
//enum ActionUpdate {
//    Result,
//    Start,
//    Stop,
//    Msg,
//}
//

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

pub type JobId = u64;

#[derive(Clone, Debug, Default)]
pub struct JobsStateInner {
    pub jid_to_job: HashMap<JobId, BuildJob>,
    pub drv_to_jobs: HashMap<Drv, HashSet<JobId>>,
}

#[derive(Clone, Debug, PartialEq, Hash, Eq)]
pub struct Drv {
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
                job.stop_time = Some(Instant::now());
            }
            Entry::Vacant(_vacant_entry) => {}
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
        new_job @ BuildJob { id, .. }: BuildJob,
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
    loop {
        tokio::select! {
            res = listener.accept() => {
                match res {
                    Ok((stream, _addr))
                        if !is_shutdown.load(Ordering::Relaxed) =>
                        {
                            let is_shutdown_ = is_shutdown.clone();
                            let cur_state_ = cur_state.clone();
                            tokio::spawn(async move {
                                if let Err(e)
                                    = read_stream(
                                        stream,
                                        cur_state_,
                                        is_shutdown_
                                        ).await {
                                    eprintln!("client error: {e}");
                                }
                            });
                        }
                    Ok((_stream, _addr)) => {
                    }

                    Err(e) => eprintln!("accept error: {e}"),
                }
            }
            _ = ticker.tick() => {
                    let tmp_state = cur_state.read().await;
                    if info_builds.send(tmp_state.clone()).is_err() {
                        eprintln!("no active receivers for info_builds");
                    }
                }
        }

        if is_shutdown.load(Ordering::Relaxed) {
            break;
        }
    }
}

//#[derive(Clone, Debug)]
//pub enum BuildPhaseType {
//    UnpackPhase,
//    PatchPhase,
//    ConfigurePhase,
//    BuildPhase,
//    CheckPhase,
//    InstallPhase,
//    FixupPhase,
//    InstallCheckPhase,
//    DistPhase,
//}

#[derive(Clone, Debug)]
pub struct BuildJob {
    pub id: u64,
    pub drv: Drv,
    pub status: JobStatus,
    pub start_time: Instant,
    pub stop_time: Option<Instant>,
}

impl BuildJob {
    pub fn new(id: u64, drv: Drv) -> Self {
        BuildJob {
            id,
            drv,
            status: JobStatus::Starting,
            start_time: Instant::now(),
            stop_time: None,
        }
    }

    pub fn runtime(&self) -> Duration {
        if let Some(stop_time) = self.stop_time {
            stop_time - self.start_time
        } else {
            self.start_time.elapsed()
        }
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

#[derive(Clone, Debug)]
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
    NotEnoughInfo,
}

impl JobStatus {
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
                write!(f, "Finished, but wasn't able to infer task type ")
            }
            JobStatus::Downloading {
                cache_name,
                narinfo_name,
            } => {
                write!(f, "Downloading {narinfo_name} from {cache_name}")
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

pub fn format_duration(dur: Duration) -> String {
    let secs = dur.as_secs();
    format_secs(secs)
}

async fn handle_line(line: String, state: JobsState) {
    match LogMessage::from_json_str(&line) {
        Ok(msg) => {
            //println!("{:?}", msg);
            let msg_ = msg.clone();
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
                        if let Some(drv) = parse_to_str(fields.as_ref(), 0) {
                            let drv = parse_drv(drv);
                            let new_job = BuildJob::new(id, drv);
                            state.replace_build_job(new_job).await;
                        } else {
                            // TODO proper logging using the usual crate
                            eprintln!(
                                "Error on either getting the fields, or that \
                                 the fields are not valid utf8. Msg in \
                                 question: {}",
                                msg_
                            );
                        }
                    }
                    ActivityType::OptimiseStore => {}
                    ActivityType::VerifyPaths => (),
                    ActivityType::Substitute => (),
                    ActivityType::QueryPathInfo => {
                        let fr = fields.as_ref();
                        if let (Some(drv), Some(cache)) =
                            (parse_to_str(fr, 0), parse_to_str(fr, 1))
                        {
                            let drv = parse_drv(drv);
                            let new_job = BuildJob {
                                status: JobStatus::Querying(cache),
                                ..BuildJob::new(id, drv)
                            };
                            state.replace_build_job(new_job).await;
                        }
                    }
                    ActivityType::PostBuildHook => {}
                    ActivityType::BuildWaiting => (),
                    ActivityType::FetchTree => (),
                },
                LogMessage::Stop { id } => {
                    state.stop_build_job(id).await;
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
                                .mutate_build_job(id, move |job| {
                                    job.status =
                                        JobStatus::BuildPhaseType(phase_name);
                                })
                                .await;
                        } else {
                            eprintln!(
                                "Error on either getting the fields, or that \
                                 the fields are not valid utf8. Msg in \
                                 question: {}",
                                msg_
                            );
                        }
                    }
                    json_parsing_nix::ResultType::Progress => (),
                    json_parsing_nix::ResultType::SetExpected => (),
                    json_parsing_nix::ResultType::PostBuildLogLine => (),
                    json_parsing_nix::ResultType::FetchStatus => (),
                },
                LogMessage::Msg { .. } => {}
                LogMessage::SetPhase { .. } => {}
            }
        }
        Err(e) => {
            eprintln!("JSON error: {e}\n\tline was: {line}")
        }
    }
}

fn parse_drv(drv: String) -> Drv {
    let s = drv.strip_prefix("/nix/store/").unwrap_or(&drv);

    match s.split_once('-') {
        Some((hash, name)) => Drv {
            hash: hash.to_string(),
            name: name.to_string(),
        },
        None => Drv {
            hash: s.to_string(),
            name: String::new(),
        },
    }
}

async fn read_stream(
    stream: UnixStream,
    state: JobsState,
    //info_builds: watch::Sender<HashMap<u64, BuildJob>>,
    is_shutdown: Arc<AtomicBool>,
) -> io::Result<()> {
    let reader = BufReader::new(stream);
    let mut lines = reader.lines();

    loop {
        if is_shutdown.load(Ordering::Relaxed) {
            return Ok(());
        }
        if let Ok(Some(line)) = lines.next_line().await {
            handle_line(line, state.clone()).await;
        }
    }
}
