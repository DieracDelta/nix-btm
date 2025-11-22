use std::sync::Arc;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::RwLock,
};

use crate::{
    daemon_side::create_shmem_and_write_snapshot,
    handle_internal_json::JobsStateInner,
    protocol_common::ProtocolError,
    ring_writer::RingWriter,
    rpc::{
        ClientRequest, DaemonResponse, deserialize_message, serialize_message,
    },
};

/// Handle a single RPC connection from a client
pub async fn handle_rpc_connection(
    mut stream: UnixStream,
    ring_writer: Arc<RwLock<RingWriter>>,
    state: Arc<RwLock<JobsStateInner>>,
) -> Result<(), ProtocolError> {
    let mut buf = vec![0u8; 8192];
    let mut _snapshot_holder = None; // Keep snapshots alive

    loop {
        // Read request
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            // Connection closed
            return Ok(());
        }

        let request_buf = &buf[..n];
        let (request, _) =
            match deserialize_message::<ClientRequest>(request_buf)? {
                Some(r) => r,
                None => continue, // Need more data
            };

        // Process request
        let response = match request {
            ClientRequest::RequestRing => {
                let writer = ring_writer.read().await;
                let total_len = std::mem::size_of::<
                    crate::protocol_common::ShmHeader,
                >() as u64
                    + writer.ring_len as u64;

                DaemonResponse::RingReady {
                    ring_name: writer.name.clone(),
                    total_len,
                }
            }
            ClientRequest::RequestSnapshot { client_pid } => {
                // Drop old snapshot before creating new one to avoid name
                // collision
                drop(_snapshot_holder.take());

                let state_guard = state.read().await;

                // Get current sequence number from ring writer
                let writer = ring_writer.read().await;
                let hv = writer.header_view();
                let (snap_seq, _) = hv.read_seq_and_next_entry_offset();

                // Create snapshot
                match create_shmem_and_write_snapshot(
                    &state_guard,
                    snap_seq as u64,
                    client_pid,
                ) {
                    Ok(snapshot) => {
                        let snapshot_name =
                            format!("nix-btm-snapshot-p{}", client_pid);
                        let response = DaemonResponse::SnapshotReady {
                            snapshot_name,
                            total_len: snapshot.total_len_bytes,
                            snap_seq: snapshot.snap_seq_uid,
                        };
                        // Keep snapshot alive until connection closes
                        _snapshot_holder = Some(snapshot);
                        response
                    }
                    Err(e) => DaemonResponse::Error {
                        message: format!("Failed to create snapshot: {:?}", e),
                    },
                }
            }
        };

        // Send response
        let response_bytes = serialize_message(&response)?;
        stream.write_all(&response_bytes).await?;
        stream.flush().await?;
    }
}
