//! Minimal CLI for querying the nix-analyticsd control socket.
//!
//! Usage:
//!   nix-analytics-ctl snapshot
//!   nix-analytics-ctl list-builds
//!   nix-analytics-ctl list-machines
//!   nix-analytics-ctl get-history

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use nix_analytics_common::protocol::{self, Request, Response};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("snapshot");

    let request = match cmd {
        "snapshot" => Request::GetSnapshot,
        "list-builds" => Request::ListBuilds,
        "list-machines" => Request::ListMachines,
        "get-history" => Request::GetHistory { last_n: 100 },
        other => anyhow::bail!("unknown command: {other}"),
    };

    let mut stream = UnixStream::connect(protocol::DEFAULT_CONTROL_SOCKET)
        .await
        .context("connecting to control socket")?;

    let request_bytes = protocol::encode_message(&request)?;
    stream.write_all(&request_bytes).await?;

    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;

    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await?;

    let response: Response = protocol::decode_message(&payload)?;

    let json = serde_json::to_string_pretty(&response)?;
    println!("{json}");

    Ok(())
}
