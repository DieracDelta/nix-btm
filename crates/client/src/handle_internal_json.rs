use std::{
    collections::{HashMap, hash_map::Entry},
    fmt::Display,
    fs, io,
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

pub async fn handle_daemon_info(
    socket_path: PathBuf,
    mode: u32,
    is_shutdown: Arc<AtomicBool>,
    info_builds: watch::Sender<HashMap<u64, BuildJob>>,
) {
    // Call the socket function **once**
    let (listener, _guard) = setup_unix_socket(&socket_path, mode)
        .expect("setup_unix_socket failed");

    let mut ticker = interval(Duration::from_secs(1));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let cur_state = Arc::new(RwLock::new(Default::default()));
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
                            }

                            );
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
    pub drv_name: String,
    pub drv_hash: String,
    pub status: JobStatus,
    pub start_time: Instant,
}

#[derive(Clone, Debug)]
pub enum JobStatus {
    BuildPhaseType(String),
    Starting,
}

impl Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobStatus::BuildPhaseType(s) => write!(f, "Progress: {}", s),
            JobStatus::Starting => write!(f, "Starting"),
        }
    }
}

pub fn format_duration(dur: Duration) -> String {
    let secs = dur.as_secs();

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

async fn handle_line(
    line: String,
    locked_state: Arc<RwLock<HashMap<u64, BuildJob>>>,
) {
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
                        if let Some(drv) = fields
                            .as_ref()
                            .and_then(|v| v.first())
                            .and_then(|f| match f {
                                Field::String(cow) => Some(
                                    cow.as_ref().to_str_lossy().into_owned(),
                                ),
                                _ => None,
                            })
                        {
                            let (drv_hash, drv_name) = parse_drv(drv);
                            let new_job = BuildJob {
                                id,
                                drv_name,
                                drv_hash,
                                status: JobStatus::Starting,
                                start_time: Instant::now(),
                            };
                            // TODO may want to batch eventually
                            // or lock more coarsely
                            // or maintain a set of diffs and then "merge"
                            // that every few
                            // seconds
                            let mut state = locked_state.write().await;
                            match state.entry(id) {
                                Entry::Occupied(mut occupied_entry) => {
                                    eprintln!(
                                        "warning: job for {id:?} already \
                                         existed; replacing it"
                                    );
                                    occupied_entry.insert(new_job);
                                }
                                Entry::Vacant(vacant_entry) => {
                                    vacant_entry.insert(new_job);
                                }
                            }
                        } else {
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
                    ActivityType::QueryPathInfo => {}
                    ActivityType::PostBuildHook => {}
                    ActivityType::BuildWaiting => (),
                    ActivityType::FetchTree => (),
                },
                LogMessage::Stop { .. } => {}
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
                            // TODO separate out into a replace_entry
                            // function
                            let mut state = locked_state.write().await;
                            match state.entry(id) {
                                Entry::Occupied(mut occupied_entry) => {
                                    //eprintln!(
                                    //    "warning: job for {id:?} already \
                                    //     existed; replacing it"
                                    //);
                                    let job = occupied_entry.get_mut();
                                    job.status =
                                        JobStatus::BuildPhaseType(phase_name);
                                }
                                Entry::Vacant(_vacant_entry) => {
                                    eprintln!("job doesn't exist?");
                                }
                            }
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

fn parse_drv(drv: String) -> (String, String) {
    let s = drv.strip_prefix("/nix/store/").unwrap_or(&drv);

    match s.split_once('-') {
        Some((hash, name)) => (hash.to_string(), name.to_string()),
        None => (s.to_string(), String::new()),
    }
}

async fn read_stream(
    stream: UnixStream,
    state: Arc<RwLock<HashMap<u64, BuildJob>>>,
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
