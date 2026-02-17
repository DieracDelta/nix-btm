//! Listens on the event Unix socket for events from the C++ plugin.

use std::os::unix::fs::PermissionsExt;

use anyhow::{Context, Result};
use tokio::io::AsyncReadExt;
use tokio::net::UnixListener;

use nix_btm_common::event::BtmEvent;

use crate::state::SharedState;

/// Run the event listener loop.
///
/// Accepts connections from the plugin on the given socket path and processes
/// events. The plugin opens a new connection from each daemon fork, so we
/// handle multiple concurrent connections.
pub async fn run(socket_path: impl AsRef<std::path::Path>, state: SharedState) -> Result<()> {
    let socket_path = socket_path.as_ref();

    // Clean up stale socket file.
    let _ = std::fs::remove_file(socket_path);

    // Ensure parent directory exists.
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }

    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("binding to {}", socket_path.display()))?;

    // Allow non-root users (plugin, TUI) to connect to the event socket.
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o666))
        .with_context(|| format!("setting permissions on {}", socket_path.display()))?;

    tracing::info!(path = %socket_path.display(), "event listener ready");

    loop {
        let (stream, _addr) = listener.accept().await?;
        let state = state.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, state).await {
                tracing::warn!("event connection error: {e}");
            }
        });
    }
}

async fn handle_connection(
    mut stream: tokio::net::UnixStream,
    state: SharedState,
) -> Result<()> {
    let mut len_buf = [0u8; 4];

    loop {
        // Read 4-byte length prefix.
        match stream.read_exact(&mut len_buf).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e.into()),
        }

        let len = u32::from_be_bytes(len_buf) as usize;
        if len > 1024 * 1024 {
            anyhow::bail!("event message too large: {len} bytes");
        }

        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).await?;

        match serde_json::from_slice::<BtmEvent>(&payload) {
            Ok(event) => {
                tracing::trace!(?event, "received event");
                state.handle_event(event).await;
            }
            Err(e) => {
                tracing::warn!("failed to decode event: {e}");
            }
        }
    }
}
