use std::{
    fs, io,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    thread::JoinHandle,
};

use either::Either;
use json_parsing_nix::LogMessage;
use serde::Deserialize;
use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    net::{UnixListener, UnixStream},
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

pub fn handle_daemon_info(socket_path: PathBuf, mode: u32) {
    let _ = std::thread::Builder::new()
        .name("io-thread".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build Tokio runtime");

            rt.block_on(async move {
                // Call the socket function **once**
                let (listener, guard) = setup_unix_socket(&socket_path, mode)
                    .expect("setup_unix_socket failed");

                loop {
                    match listener.accept().await {
                        Ok((stream, _addr)) => {
                            tokio::spawn(async move {
                                if let Err(e) = read_stream(stream).await {
                                    eprintln!("client error: {e}");
                                }
                            });
                        }
                        Err(e) => eprintln!("accept error: {e}"),
                    }
                }
            });
        })
        .unwrap();
}

async fn read_stream(stream: UnixStream) -> io::Result<()> {
    let reader = BufReader::new(stream);
    let mut lines = reader.lines();

    while let Some(line) = lines.next_line().await? {
        let line_with_prefix = format!("@nix {}", line);
        match LogMessage::from_json_str(&line_with_prefix) {
            Ok(msg) => {
                match msg {
                    LogMessage::Start {
                        fields,
                        id,
                        level,
                        parent,
                        text,
                        r#type,
                    } => todo!(),
                    LogMessage::Stop { id } => todo!(),
                    LogMessage::Result { fields, id, r#type } => todo!(),
                    LogMessage::Msg { level, msg } => todo!(),
                    LogMessage::SetPhase { phase } => todo!(),
                }
                println!("{:?}", msg)
            }
            Err(e) => {
                eprintln!("JSON error: {e}\n\tline was: {line}")
            }
        }
    }

    Ok(())
}

//type ActivityId = u64;

//#[derive(Debug, Deserialize)]
//#[repr(i32)]
//enum ActivityTypeTag {
//    Unknown = 0,
//    CopyPath = 100,
//    FileTransfer = 101,
//    Realise = 102,
//    CopyPaths = 103,
//    Builds = 104,
//    Build = 105,
//    OptimiseStore = 106,
//    VerifyPaths = 107,
//    Substitute = 108,
//    QueryPathInfo = 109,
//    PostBuildHook = 110,
//    BuildWaiting = 111,
//    FetchTree = 112,
//}
//
//#[repr(i32)]
//#[derive(Copy, Clone, Debug, Eq, PartialEq)]
//pub enum NixVerbosity {
//    LvlError = 0,
//    LvlWarn,
//    LvlNotice,
//    LvlInfo,
//    LvlTalkative,
//    LvlChatty,
//    LvlDebug,
//    LvlVomit,
//}
//
//pub struct Activity {
//    tag: ActivityTypeTag,
//    activity_fields: ActivityUpdateFields,
//}

//#[derive(Debug)]
//pub enum ActivityUpdateFields {
//    CopyPath {
//        source_store_path: String,
//        src_uri: String,
//        dst_uri: String,
//    }, /* {storePathS, srcCfg.getHumanReadableURI(),
//        * dstCfg.getHumanReadableURI()}); */
//    FileTransfer {
//        uri: String,
//    },
//    Realise, //
//    CopyPaths,
//    Builds {
//        phase_type: String,
//    },
//    BuildStart {
//        drv_path: String,
//        // TODO not sure what these are for...
//        _empty_str: String,
//        _one_1: i32,
//        _one_2: i32,
//    },
//    BuildUpdate {
//        a: u64,
//        b: u64,
//        c: u64,
//        d: u64,
//    },
//    OptimiseStore,
//    VerifyPaths,
//    Substitute,
//    QueryPathInfo,
//    PostBuildHook {
//        store_path: String,
//    },
//    BuildWaiting {
//        store_path: String,
//        resolved_path: String,
//    },
//
//    FetchTree,
//}
