//! Full pipeline integration test: event listener + control server on shared state.
//!
//! Simulates a complete build lifecycle and queries the result via the control socket.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use nix_btm_common::event::BtmEvent;
use nix_btm_common::protocol::{self, Request, Response};
use nix_btmd::control;
use nix_btmd::event_listener;
use nix_btmd::state::SharedState;

async fn wait_for_socket(path: &str, timeout: Duration) -> UnixStream {
    let start = tokio::time::Instant::now();
    loop {
        match UnixStream::connect(path).await {
            Ok(stream) => return stream,
            Err(_) if start.elapsed() < timeout => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(e) => panic!("socket {path} not ready after {timeout:?}: {e}"),
        }
    }
}

async fn send_event(stream: &mut UnixStream, event: &BtmEvent) {
    let json = serde_json::to_vec(event).unwrap();
    let len = (json.len() as u32).to_be_bytes();
    stream.write_all(&len).await.unwrap();
    stream.write_all(&json).await.unwrap();
}

async fn control_roundtrip(stream: &mut UnixStream, request: &Request) -> Response {
    let bytes = protocol::encode_message(request).unwrap();
    stream.write_all(&bytes).await.unwrap();

    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await.unwrap();
    let len = u32::from_be_bytes(len_buf) as usize;

    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await.unwrap();

    protocol::decode_message(&payload).unwrap()
}

#[tokio::test]
async fn complete_build_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let event_sock = dir.path().join("events.sock");
    let control_sock = dir.path().join("control.sock");
    let event_str = event_sock.to_str().unwrap().to_string();
    let control_str = control_sock.to_str().unwrap().to_string();

    let state = SharedState::new();

    // Spawn event listener.
    let event_state = state.clone();
    let _event_handle = tokio::spawn(async move {
        event_listener::run(&event_str, event_state).await.ok();
    });

    // Spawn control server.
    let control_state = state.clone();
    let _control_handle = tokio::spawn(async move {
        control::run(&control_str, control_state).await.ok();
    });

    // Connect as plugin.
    let mut plugin =
        wait_for_socket(event_sock.to_str().unwrap(), Duration::from_secs(5)).await;

    // Simulate full build lifecycle.
    // 1. ActivityStarted
    send_event(
        &mut plugin,
        &BtmEvent::ActivityStarted {
            timestamp_us: 1000,
            activity_id: 42,
            activity_type: 105,
            description: "building hello-1.0".to_string(),
            drv_path: Some("/nix/store/abc-hello-1.0.drv".to_string()),
            parent_id: 0,
            command_line: None,
            nix_pid: None,
        },
    )
    .await;

    // 2. PhaseChanged
    send_event(
        &mut plugin,
        &BtmEvent::PhaseChanged {
            timestamp_us: 1100,
            activity_id: 42,
            phase: "unpackPhase".to_string(),
        },
    )
    .await;

    // 3. LogLine
    send_event(
        &mut plugin,
        &BtmEvent::LogLine {
            timestamp_us: 1200,
            activity_id: 42,
            text: "unpacking source".to_string(),
        },
    )
    .await;

    // 4. Progress
    send_event(
        &mut plugin,
        &BtmEvent::Progress {
            timestamp_us: 1300,
            activity_id: 42,
            done: 1,
            expected: 5,
            running: 1,
            failed: 0,
        },
    )
    .await;

    // 5. ActivityStopped
    send_event(
        &mut plugin,
        &BtmEvent::ActivityStopped {
            timestamp_us: 2000,
            activity_id: 42,
        },
    )
    .await;

    // Let events propagate.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Now query via control socket.
    let mut tui =
        wait_for_socket(control_sock.to_str().unwrap(), Duration::from_secs(5)).await;

    // Should have no active builds (it completed).
    let resp = control_roundtrip(&mut tui, &Request::ListBuilds).await;
    match resp {
        Response::Builds { builds } => assert!(builds.is_empty()),
        other => panic!("expected Builds, got {other:?}"),
    }

    // Should have one history entry.
    let resp = control_roundtrip(&mut tui, &Request::GetHistory { last_n: 10 }).await;
    match resp {
        Response::History { builds } => {
            assert_eq!(builds.len(), 1);
            let completed = &builds[0];
            assert_eq!(completed.build.activity_id, 42);
            assert!(completed.success);
            assert_eq!(completed.finished_at_us, 2000);
            assert_eq!(completed.build.description, "building hello-1.0");
        }
        other => panic!("expected History, got {other:?}"),
    }

    // Full snapshot check.
    let resp = control_roundtrip(&mut tui, &Request::GetSnapshot).await;
    match resp {
        Response::Snapshot { snapshot } => {
            assert!(snapshot.active_builds.is_empty());
            assert_eq!(snapshot.recent_history.len(), 1);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }
}
