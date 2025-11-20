use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::protocol_common::ProtocolError;
use crate::rpc::{ClientRequest, DaemonResponse, deserialize_message, serialize_message};

/// Send an RPC request to the daemon and receive the response
pub async fn send_rpc_request(
    stream: &mut UnixStream,
    request: ClientRequest,
) -> Result<DaemonResponse, ProtocolError> {
    // Serialize and send request
    let request_bytes = serialize_message(&request)?;
    stream.write_all(&request_bytes).await?;
    stream.flush().await?;

    // Read response data
    let mut buf = vec![0u8; 8192];
    let n = stream.read(&mut buf).await?;
    buf.truncate(n);

    let (response, _) = deserialize_message::<DaemonResponse>(&buf)?
        .ok_or_else(|| ProtocolError::MisMatchError {
            backtrace: std::backtrace::Backtrace::capture(),
        })?;

    Ok(response)
}
