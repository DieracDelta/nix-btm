use std::{
    fs, io,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    thread::JoinHandle,
};

use serde::Deserialize;
use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    net::{UnixListener, UnixStream},
};

#[derive(Debug, Deserialize)]
pub struct LogLine {
    pub action: String,
    #[serde(default)]
    pub fields: Vec<String>,
    pub id: u64,
    #[serde(rename = "type")]
    pub kind: u32,
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
        match serde_json::from_str::<LogLine>(&line) {
            Ok(msg) => println!("{:?}", msg),
            Err(e) => eprintln!("JSON error: {e}\n{line}"),
        }
    }

    Ok(())
}
