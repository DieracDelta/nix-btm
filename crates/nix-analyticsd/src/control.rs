//! Control server: handles queries and commands from the TUI.

use anyhow::{Context, Result};
use nix::sys::signal::Signal as NixSignal;
use nix::unistd::Pid;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;

use nix_analytics_common::protocol::{self, BuildAction, Request, Response};

use crate::state::SharedState;

/// Run the control server loop.
pub async fn run(socket_path: &str, state: SharedState) -> Result<()> {
    let _ = std::fs::remove_file(socket_path);

    if let Some(parent) = std::path::Path::new(socket_path).parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }

    let listener =
        UnixListener::bind(socket_path).with_context(|| format!("binding to {socket_path}"))?;

    tracing::info!("control server ready on {socket_path}");

    loop {
        let (stream, _addr) = listener.accept().await?;
        let state = state.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, state).await {
                tracing::warn!("control client error: {e}");
            }
        });
    }
}

async fn handle_client(
    mut stream: tokio::net::UnixStream,
    state: SharedState,
) -> Result<()> {
    let mut len_buf = [0u8; 4];

    loop {
        match stream.read_exact(&mut len_buf).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e.into()),
        }

        let len = u32::from_be_bytes(len_buf) as usize;
        if len > 1024 * 1024 {
            anyhow::bail!("request too large: {len} bytes");
        }

        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).await?;

        let request: Request = match protocol::decode_message(&payload) {
            Ok(req) => req,
            Err(e) => {
                tracing::warn!("failed to decode request: {e}");
                let resp = Response::Error {
                    message: format!("invalid request: {e}"),
                };
                let response_bytes = protocol::encode_message(&resp)?;
                stream.write_all(&response_bytes).await?;
                continue;
            }
        };
        let response = handle_request(&state, request).await;

        let response_bytes = protocol::encode_message(&response)?;
        stream.write_all(&response_bytes).await?;
    }
}

async fn handle_request(state: &SharedState, request: Request) -> Response {
    match request {
        Request::GetSnapshot => Response::Snapshot {
            snapshot: state.snapshot().await,
        },

        Request::ListBuilds => Response::Builds {
            builds: state.list_builds().await,
        },

        Request::GetBuild { activity_id } => Response::Build {
            build: state.get_build(activity_id).await.map(Box::new),
        },

        Request::GetBuildLog {
            activity_id,
            last_n,
        } => Response::BuildLog {
            activity_id,
            lines: state.get_build_log(activity_id, last_n).await,
        },

        Request::ListMachines => Response::Machines {
            machines: state.list_machines().await,
        },

        Request::GetHistory { last_n } => Response::History {
            builds: state.get_history(last_n).await,
        },

        Request::ControlBuild {
            activity_id,
            action,
        } => match control_build(state, activity_id, action).await {
            Ok(()) => Response::Ok,
            Err(e) => Response::Error {
                message: e.to_string(),
            },
        },

        Request::SetNice {
            activity_id,
            weight,
        } => match set_cgroup_value(state, activity_id, "cpu.weight", &weight.to_string()).await {
            Ok(()) => Response::Ok,
            Err(e) => Response::Error {
                message: e.to_string(),
            },
        },

        Request::SetCpuLimit {
            activity_id,
            percent,
        } => {
            // cpu.max format: "$MAX $PERIOD" where period is typically 100000us.
            // percent=200 means 2 cores → "200000 100000"
            let max = (percent as u64) * 1000;
            let value = format!("{max} 100000");
            match set_cgroup_value(state, activity_id, "cpu.max", &value).await {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error {
                    message: e.to_string(),
                },
            }
        }

        Request::SetMemoryLimit {
            activity_id,
            bytes,
        } => match set_cgroup_value(state, activity_id, "memory.max", &bytes.to_string()).await {
            Ok(()) => Response::Ok,
            Err(e) => Response::Error {
                message: e.to_string(),
            },
        },
    }
}

async fn control_build(
    state: &SharedState,
    activity_id: u64,
    action: BuildAction,
) -> Result<()> {
    // Path 1: cgroup (per-build, atomic)
    if let Some(cgroup_path) = state.get_cgroup_path(activity_id).await {
        control_build_cgroup(&cgroup_path, activity_id, action).await?;
    } else {
        // Path 2: PID fallback (per-nix-invocation)
        let pid = state.get_user_pid(activity_id).await.context(
            "build has no cgroup and no nix_pid (is use-cgroups enabled? is plugin up to date?)",
        )?;
        control_build_pid(pid, activity_id, action).await?;
    }

    // Update frozen state in shared state.
    match action {
        BuildAction::Freeze => state.set_frozen(activity_id, true).await,
        BuildAction::Unfreeze => state.set_frozen(activity_id, false).await,
        BuildAction::Kill => {}
    }

    Ok(())
}

async fn write_cgroup_file(path: &std::path::Path, value: &[u8]) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut f = tokio::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .await
        .with_context(|| format!("opening {}", path.display()))?;
    f.write_all(value)
        .await
        .with_context(|| format!("writing to {}", path.display()))?;
    Ok(())
}

async fn control_build_cgroup(
    cgroup_path: &std::path::Path,
    activity_id: u64,
    action: BuildAction,
) -> Result<()> {
    match action {
        BuildAction::Kill => {
            write_cgroup_file(&cgroup_path.join("cgroup.kill"), b"1").await?;
        }
        BuildAction::Freeze => {
            write_cgroup_file(&cgroup_path.join("cgroup.freeze"), b"1").await?;
        }
        BuildAction::Unfreeze => {
            write_cgroup_file(&cgroup_path.join("cgroup.freeze"), b"0").await?;
        }
    }
    tracing::info!(?activity_id, ?cgroup_path, ?action, "controlled build via cgroup");
    Ok(())
}

async fn control_build_pid(pid: u32, activity_id: u64, action: BuildAction) -> Result<()> {
    let signal = match action {
        BuildAction::Kill => NixSignal::SIGKILL,
        BuildAction::Freeze => NixSignal::SIGSTOP,
        BuildAction::Unfreeze => NixSignal::SIGCONT,
    };
    nix::sys::signal::kill(Pid::from_raw(pid as i32), signal)
        .with_context(|| format!("sending {signal:?} to pid {pid}"))?;
    tracing::warn!(
        activity_id,
        pid,
        "PID fallback: affects entire nix invocation, not just this build"
    );
    Ok(())
}

async fn set_cgroup_value(
    state: &SharedState,
    activity_id: u64,
    filename: &str,
    value: &str,
) -> Result<()> {
    let cgroup_path = state
        .get_cgroup_path(activity_id)
        .await
        .context("build has no cgroup (is use-cgroups enabled?)")?;

    let file_path = cgroup_path.join(filename);
    write_cgroup_file(&file_path, value.as_bytes()).await?;

    tracing::info!(?activity_id, filename, value, "set cgroup value");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix_analytics_common::event::AnalyticsEvent;
    use nix_analytics_common::protocol::BuildAction;

    fn make_started_event(id: u64) -> AnalyticsEvent {
        AnalyticsEvent::ActivityStarted {
            timestamp_us: 1000,
            activity_id: id,
            activity_type: 105,
            description: format!("building test-{id}"),
            drv_path: Some(format!("/nix/store/abc-test-{id}.drv")),
            parent_id: 0,
            command_line: None,
            nix_pid: None,
        }
    }

    #[tokio::test]
    async fn handle_get_snapshot() {
        let state = SharedState::new();
        state.handle_event(make_started_event(1)).await;
        let resp = handle_request(&state, Request::GetSnapshot).await;
        match resp {
            Response::Snapshot { snapshot } => {
                assert_eq!(snapshot.active_builds.len(), 1);
            }
            other => panic!("expected Snapshot, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_list_builds() {
        let state = SharedState::new();
        state.handle_event(make_started_event(1)).await;
        state.handle_event(make_started_event(2)).await;
        let resp = handle_request(&state, Request::ListBuilds).await;
        match resp {
            Response::Builds { builds } => assert_eq!(builds.len(), 2),
            other => panic!("expected Builds, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_get_build_found() {
        let state = SharedState::new();
        state.handle_event(make_started_event(42)).await;
        let resp = handle_request(&state, Request::GetBuild { activity_id: 42 }).await;
        match resp {
            Response::Build { build } => {
                assert!(build.is_some());
                assert_eq!(build.unwrap().activity_id, 42);
            }
            other => panic!("expected Build, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_get_build_not_found() {
        let state = SharedState::new();
        let resp = handle_request(&state, Request::GetBuild { activity_id: 999 }).await;
        match resp {
            Response::Build { build } => assert!(build.is_none()),
            other => panic!("expected Build, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_get_build_log() {
        let state = SharedState::new();
        state.handle_event(make_started_event(1)).await;
        state
            .handle_event(AnalyticsEvent::LogLine {
                timestamp_us: 1500,
                activity_id: 1,
                text: "hello".to_string(),
            })
            .await;
        let resp = handle_request(
            &state,
            Request::GetBuildLog {
                activity_id: 1,
                last_n: 100,
            },
        )
        .await;
        match resp {
            Response::BuildLog { lines, .. } => assert_eq!(lines, vec!["hello"]),
            other => panic!("expected BuildLog, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_list_machines() {
        let state = SharedState::new();
        let resp = handle_request(&state, Request::ListMachines).await;
        match resp {
            Response::Machines { machines } => assert!(machines.is_empty()),
            other => panic!("expected Machines, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_get_history() {
        let state = SharedState::new();
        state.handle_event(make_started_event(1)).await;
        state
            .handle_event(AnalyticsEvent::ActivityStopped {
                timestamp_us: 2000,
                activity_id: 1,
            })
            .await;
        let resp = handle_request(&state, Request::GetHistory { last_n: 10 }).await;
        match resp {
            Response::History { builds } => {
                assert_eq!(builds.len(), 1);
                assert_eq!(builds[0].build.activity_id, 1);
            }
            other => panic!("expected History, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_control_build_no_cgroup_no_pid() {
        let state = SharedState::new();
        state.handle_event(make_started_event(1)).await;
        let resp = handle_request(
            &state,
            Request::ControlBuild {
                activity_id: 1,
                action: BuildAction::Kill,
            },
        )
        .await;
        match resp {
            Response::Error { message } => {
                assert!(message.contains("no cgroup"));
                assert!(message.contains("no nix_pid"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }
}
