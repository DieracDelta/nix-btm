use serde::{Deserialize, Serialize};

/// Client requests to the daemon
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientRequest {
    /// Request access to the ring buffer shared memory
    RequestRing,
    /// Request a snapshot of the current state
    /// Includes the client's PID for naming the snapshot shared memory
    RequestSnapshot { client_pid: i32 },
}

/// Daemon responses to client requests
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonResponse {
    /// Ring buffer is ready at the given shared memory name
    /// Client should open: /dev/shm/{ring_name}
    RingReady { ring_name: String, total_len: u64 },
    /// Snapshot is ready in shared memory at the given name
    /// Client should open: /dev/shm/{snapshot_name}
    SnapshotReady {
        snapshot_name: String,
        total_len: u64,
        snap_seq: u64,
    },
    /// Request failed with error message
    Error { message: String },
}

/// Wire protocol: [u32 length][CBOR payload]
/// Length is in native endian (local unix socket, same machine)
pub const HEADER_SIZE: usize = std::mem::size_of::<u32>();

/// Serialize a request/response into wire format
pub fn serialize_message<T: Serialize>(
    msg: &T,
) -> Result<Vec<u8>, serde_cbor::Error> {
    let payload = serde_cbor::to_vec(msg)?;
    let len = payload.len() as u32;
    let mut buf = Vec::with_capacity(HEADER_SIZE + payload.len());
    buf.extend_from_slice(&len.to_ne_bytes());
    buf.extend_from_slice(&payload);
    Ok(buf)
}

/// Deserialize a message from wire format
/// Returns (message, bytes_consumed)
pub fn deserialize_message<T: for<'de> Deserialize<'de>>(
    buf: &[u8],
) -> Result<Option<(T, usize)>, serde_cbor::Error> {
    if buf.len() < HEADER_SIZE {
        return Ok(None); // Need more data for header
    }

    let len_bytes: [u8; 4] = buf[..HEADER_SIZE].try_into().unwrap();
    let payload_len = u32::from_ne_bytes(len_bytes) as usize;

    let total_len = HEADER_SIZE + payload_len;
    if buf.len() < total_len {
        return Ok(None); // Need more data for payload
    }

    let payload = &buf[HEADER_SIZE..total_len];
    let msg: T = serde_cbor::from_slice(payload)?;
    Ok(Some((msg, total_len)))
}
