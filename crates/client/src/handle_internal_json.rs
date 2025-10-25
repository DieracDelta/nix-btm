use std::{
    collections::{HashMap, hash_map::Entry},
    fs, io,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, atomic::{AtomicBool, Ordering}},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use bstr::ByteSlice as _;
use either::Either;
use json_parsing_nix::{ActivityType, Field, LogMessage};
use serde::Deserialize;
use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    net::{UnixListener, UnixStream},
    sync::{Mutex, watch},
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
    let (listener, guard) = setup_unix_socket(&socket_path, mode)
        .expect("setup_unix_socket failed");

    let mut cur_state = Default::default();
    let mut ticker = interval(Duration::from_secs(1));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
     loop {
        tokio::select! {
            res = listener.accept() => {
                match res {
                    Ok((stream, _addr))
                        if !is_shutdown.load(Ordering::Relaxed) =>
                    {
                        if let Err(e) = read_stream(stream, &mut cur_state).await {
                            eprintln!("client error: {e}");
                        }
                    }
                    Ok((_stream, _addr)) => {
                        // shutdown triggered, ignore
                    }
                    Err(e) => eprintln!("accept error: {e}"),
                }
            }

            _ = ticker.tick() => {
                if info_builds.send(cur_state.clone()).is_err() {
                    // all receivers dropped, can ignore or break
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
    id: u64,
    drv: String,
    status: JobStatus,
    start_time: Instant,
}

#[derive(Clone, Debug)]
pub enum JobStatus {
    BuildPhaseType(String),
    Starting,
}

async fn read_stream(
    stream: UnixStream,
    state: &mut HashMap<u64, BuildJob>,
) -> io::Result<()> {
    let reader = BufReader::new(stream);
    let mut lines = reader.lines();

    while let Some(line) = lines.next_line().await? {
        match LogMessage::from_json_str(&line) {
            Ok(msg) => {
                //println!("{:?}", msg);
                let msg_ = msg.clone();
                match msg {
                    LogMessage::Start {
                        fields,
                        id,
                        level,
                        parent,
                        text,
                        r#type,
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
                                        cow.as_ref()
                                            .to_str_lossy()
                                            .into_owned(),
                                    ),
                                    _ => None,
                                })
                            {
                                let new_job = BuildJob {
                                    id,
                                    drv,
                                    status: JobStatus::Starting,
                                    start_time: Instant::now(),
                                };
                                // TODO may want to batch eventually
                                // or lock more coarsely
                                // or maintain a set of diffs and then "merge"
                                // that every few
                                // seconds
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
                                    "Error on either getting the fields, or \
                                     that the fields are not valid utf8. Msg \
                                     in question: {}",
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
                    LogMessage::Stop { id } => {}
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
                                        cow.as_ref()
                                            .to_str_lossy()
                                            .into_owned(),
                                    ),
                                    _ => None,
                                })
                            {
                                // TODO separate out into a replace_entry
                                // function
                                match state.entry(id) {
                                    Entry::Occupied(mut occupied_entry) => {
                                        eprintln!(
                                            "warning: job for {id:?} already \
                                             existed; replacing it"
                                        );
                                        let job = occupied_entry.get_mut();
                                        job.status = JobStatus::BuildPhaseType(
                                            phase_name,
                                        );
                                    }
                                    Entry::Vacant(_vacant_entry) => {
                                        eprintln!("job doesn't exist?");
                                    }
                                }
                            } else {
                                eprintln!(
                                    "Error on either getting the fields, or \
                                     that the fields are not valid utf8. Msg \
                                     in question: {}",
                                    msg_
                                );
                            }
                        }
                        json_parsing_nix::ResultType::Progress => (),
                        json_parsing_nix::ResultType::SetExpected => (),
                        json_parsing_nix::ResultType::PostBuildLogLine => (),
                        json_parsing_nix::ResultType::FetchStatus => (),
                    },
                    LogMessage::Msg { level, msg } => {}
                    LogMessage::SetPhase { phase } => {}
                }
            }
            Err(e) => {
                eprintln!("JSON error: {e}\n\tline was: {line}")
            }
        }
    }

    Ok(())
}
