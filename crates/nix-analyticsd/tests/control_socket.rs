//! Integration tests for the control socket server.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use nix_analytics_common::event::AnalyticsEvent;
use nix_analytics_common::protocol::{self, Request, Response};
use nix_analyticsd::control;
use nix_analyticsd::state::SharedState;

/// Wait for a Unix socket to become available, retrying with backoff.
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

/// Send a request and read back the response.
async fn roundtrip(stream: &mut UnixStream, request: &Request) -> Response {
    let bytes = protocol::encode_message(request).unwrap();
    stream.write_all(&bytes).await.unwrap();

    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await.unwrap();
    let len = u32::from_be_bytes(len_buf) as usize;

    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await.unwrap();

    protocol::decode_message(&payload).unwrap()
}

fn make_started_event(id: u64) -> AnalyticsEvent {
    AnalyticsEvent::ActivityStarted {
        timestamp_us: 1000 + id,
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
async fn list_builds_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let sock_path = dir.path().join("control.sock");
    let sock_str = sock_path.to_str().unwrap().to_string();

    let state = SharedState::new();
    state.handle_event(make_started_event(1)).await;
    state.handle_event(make_started_event(2)).await;

    let control_state = state.clone();
    let _handle = tokio::spawn(async move {
        control::run(&sock_str, control_state).await.ok();
    });

    let mut stream = wait_for_socket(sock_path.to_str().unwrap(), Duration::from_secs(5)).await;

    let resp = roundtrip(&mut stream, &Request::ListBuilds).await;
    match resp {
        Response::Builds { builds } => assert_eq!(builds.len(), 2),
        other => panic!("expected Builds, got {other:?}"),
    }
}

#[tokio::test]
async fn get_build_found_and_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let sock_path = dir.path().join("control.sock");
    let sock_str = sock_path.to_str().unwrap().to_string();

    let state = SharedState::new();
    state.handle_event(make_started_event(42)).await;

    let control_state = state.clone();
    let _handle = tokio::spawn(async move {
        control::run(&sock_str, control_state).await.ok();
    });

    let mut stream = wait_for_socket(sock_path.to_str().unwrap(), Duration::from_secs(5)).await;

    // Found.
    let resp = roundtrip(&mut stream, &Request::GetBuild { activity_id: 42 }).await;
    match resp {
        Response::Build { build } => assert!(build.is_some()),
        other => panic!("expected Build, got {other:?}"),
    }

    // Not found.
    let resp = roundtrip(&mut stream, &Request::GetBuild { activity_id: 999 }).await;
    match resp {
        Response::Build { build } => assert!(build.is_none()),
        other => panic!("expected Build, got {other:?}"),
    }
}

#[tokio::test]
async fn get_snapshot_with_populated_state() {
    let dir = tempfile::tempdir().unwrap();
    let sock_path = dir.path().join("control.sock");
    let sock_str = sock_path.to_str().unwrap().to_string();

    let state = SharedState::new();
    state.handle_event(make_started_event(1)).await;
    state.handle_event(make_started_event(2)).await;
    // Complete one build so there's history.
    state
        .handle_event(AnalyticsEvent::ActivityStopped {
            timestamp_us: 5000,
            activity_id: 1,
        })
        .await;

    let control_state = state.clone();
    let _handle = tokio::spawn(async move {
        control::run(&sock_str, control_state).await.ok();
    });

    let mut stream = wait_for_socket(sock_path.to_str().unwrap(), Duration::from_secs(5)).await;

    let resp = roundtrip(&mut stream, &Request::GetSnapshot).await;
    match resp {
        Response::Snapshot { snapshot } => {
            assert_eq!(snapshot.active_builds.len(), 1);
            assert_eq!(snapshot.recent_history.len(), 1);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }
}

#[tokio::test]
async fn multiple_requests_on_same_connection() {
    let dir = tempfile::tempdir().unwrap();
    let sock_path = dir.path().join("control.sock");
    let sock_str = sock_path.to_str().unwrap().to_string();

    let state = SharedState::new();
    state.handle_event(make_started_event(1)).await;

    let control_state = state.clone();
    let _handle = tokio::spawn(async move {
        control::run(&sock_str, control_state).await.ok();
    });

    let mut stream = wait_for_socket(sock_path.to_str().unwrap(), Duration::from_secs(5)).await;

    // First request.
    let resp = roundtrip(&mut stream, &Request::ListBuilds).await;
    assert!(matches!(resp, Response::Builds { .. }));

    // Second request on same connection.
    let resp = roundtrip(&mut stream, &Request::ListMachines).await;
    assert!(matches!(resp, Response::Machines { .. }));

    // Third request.
    let resp = roundtrip(&mut stream, &Request::GetHistory { last_n: 10 }).await;
    assert!(matches!(resp, Response::History { .. }));
}
