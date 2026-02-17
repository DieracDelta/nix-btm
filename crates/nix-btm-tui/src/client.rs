//! Client for communicating with nix-btmd over Unix socket.

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use nix_btm_common::protocol::{self, Request, Response};
use nix_btm_common::types::BtmSnapshot;

pub struct BtmClient {
    stream: UnixStream,
    socket_path: std::path::PathBuf,
}

impl BtmClient {
    pub async fn connect(socket_path: impl AsRef<std::path::Path>) -> Result<Self> {
        let socket_path = socket_path.as_ref().to_path_buf();
        let stream = UnixStream::connect(&socket_path)
            .await
            .with_context(|| format!("connecting to {}", socket_path.display()))?;
        Ok(Self {
            stream,
            socket_path,
        })
    }

    async fn send_request(&mut self, request: &Request) -> Result<Response> {
        match self.try_send_request(request).await {
            Ok(resp) => Ok(resp),
            Err(_first_err) => {
                // Connection may be broken — reconnect and retry once.
                self.stream = UnixStream::connect(&self.socket_path)
                    .await
                    .with_context(|| format!("reconnecting to {}", self.socket_path.display()))?;
                self.try_send_request(request).await
            }
        }
    }

    async fn try_send_request(&mut self, request: &Request) -> Result<Response> {
        let bytes = protocol::encode_message(request)?;
        self.stream.write_all(&bytes).await?;

        let mut len_buf = [0u8; 4];
        self.stream.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf) as usize;

        let mut payload = vec![0u8; len];
        self.stream.read_exact(&mut payload).await?;

        let response: Response = protocol::decode_message(&payload)?;
        Ok(response)
    }

    pub async fn get_snapshot(&mut self) -> Result<BtmSnapshot> {
        match self.send_request(&Request::GetSnapshot).await? {
            Response::Snapshot { snapshot } => Ok(snapshot),
            Response::Error { message } => anyhow::bail!("server error: {message}"),
            other => anyhow::bail!("unexpected response: {other:?}"),
        }
    }

    pub async fn get_build_log(&mut self, activity_id: u64, last_n: usize) -> Result<Vec<String>> {
        match self
            .send_request(&Request::GetBuildLog {
                activity_id,
                last_n,
            })
            .await?
        {
            Response::BuildLog { lines, .. } => Ok(lines),
            Response::Error { message } => anyhow::bail!("server error: {message}"),
            other => anyhow::bail!("unexpected response: {other:?}"),
        }
    }

    pub async fn control_build(
        &mut self,
        activity_id: u64,
        action: protocol::BuildAction,
    ) -> Result<()> {
        match self
            .send_request(&Request::ControlBuild {
                activity_id,
                action,
            })
            .await?
        {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("server error: {message}"),
            other => anyhow::bail!("unexpected response: {other:?}"),
        }
    }

    /// Send action to multiple builds. Returns (succeeded, errors).
    pub async fn control_builds(
        &mut self,
        ids: &[u64],
        action: protocol::BuildAction,
    ) -> (usize, Vec<String>) {
        let mut ok = 0;
        let mut errs = Vec::new();
        for &id in ids {
            match self.control_build(id, action).await {
                Ok(()) => ok += 1,
                Err(e) => errs.push(format!("build {id}: {e}")),
            }
        }
        (ok, errs)
    }

    pub async fn set_nice(&mut self, activity_id: u64, weight: u32) -> Result<()> {
        match self
            .send_request(&Request::SetNice {
                activity_id,
                weight,
            })
            .await?
        {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("server error: {message}"),
            other => anyhow::bail!("unexpected response: {other:?}"),
        }
    }

    pub async fn set_cpu_limit(&mut self, activity_id: u64, percent: u32) -> Result<()> {
        match self
            .send_request(&Request::SetCpuLimit {
                activity_id,
                percent,
            })
            .await?
        {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("server error: {message}"),
            other => anyhow::bail!("unexpected response: {other:?}"),
        }
    }

    pub async fn set_memory_limit(&mut self, activity_id: u64, bytes: u64) -> Result<()> {
        match self
            .send_request(&Request::SetMemoryLimit {
                activity_id,
                bytes,
            })
            .await?
        {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("server error: {message}"),
            other => anyhow::bail!("unexpected response: {other:?}"),
        }
    }
}
