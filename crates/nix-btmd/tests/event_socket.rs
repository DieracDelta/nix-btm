//! Integration tests for the event listener socket.

use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

use nix_btm_common::event::BtmEvent;
use nix_btmd::event_listener;
use nix_btmd::state::SharedState;

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

/// Send a length-prefixed JSON event over a UnixStream (same wire format as the C++ plugin).
async fn send_event(stream: &mut UnixStream, event: &BtmEvent) {
    let json = serde_json::to_vec(event).unwrap();
    let len = (json.len() as u32).to_be_bytes();
    stream.write_all(&len).await.unwrap();
    stream.write_all(&json).await.unwrap();
}

fn make_started_event(id: u64) -> BtmEvent {
    BtmEvent::ActivityStarted {
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
async fn basic_event_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let sock_path = dir.path().join("events.sock");
    let sock_str = sock_path.to_str().unwrap().to_string();

    let state = SharedState::new();
    let listener_state = state.clone();

    let _handle = tokio::spawn(async move {
        event_listener::run(&sock_str, listener_state).await.ok();
    });

    let mut stream = wait_for_socket(sock_path.to_str().unwrap(), Duration::from_secs(5)).await;
    send_event(&mut stream, &make_started_event(1)).await;

    // Give the handler time to process.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let builds = state.list_builds().await;
    assert_eq!(builds.len(), 1);
    assert_eq!(builds[0].activity_id, 1);
}

#[tokio::test]
async fn multiple_concurrent_connections() {
    let dir = tempfile::tempdir().unwrap();
    let sock_path = dir.path().join("events.sock");
    let sock_str = sock_path.to_str().unwrap().to_string();

    let state = SharedState::new();
    let listener_state = state.clone();

    let _handle = tokio::spawn(async move {
        event_listener::run(&sock_str, listener_state).await.ok();
    });

    let path_str = sock_path.to_str().unwrap();

    // Connect 3 "plugin forks" concurrently.
    let mut stream1 = wait_for_socket(path_str, Duration::from_secs(5)).await;
    let mut stream2 = wait_for_socket(path_str, Duration::from_secs(5)).await;
    let mut stream3 = wait_for_socket(path_str, Duration::from_secs(5)).await;

    send_event(&mut stream1, &make_started_event(1)).await;
    send_event(&mut stream2, &make_started_event(2)).await;
    send_event(&mut stream3, &make_started_event(3)).await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let builds = state.list_builds().await;
    assert_eq!(builds.len(), 3);
}

#[tokio::test]
async fn oversized_message_rejected_without_crash() {
    let dir = tempfile::tempdir().unwrap();
    let sock_path = dir.path().join("events.sock");
    let sock_str = sock_path.to_str().unwrap().to_string();

    let state = SharedState::new();
    let listener_state = state.clone();

    let _handle = tokio::spawn(async move {
        event_listener::run(&sock_str, listener_state).await.ok();
    });

    let mut stream =
        wait_for_socket(sock_path.to_str().unwrap(), Duration::from_secs(5)).await;

    // Send a length prefix claiming >1MB payload.
    let huge_len = (2 * 1024 * 1024u32).to_be_bytes();
    stream.write_all(&huge_len).await.unwrap();
    // We don't actually send the payload — the server should reject based on the length.

    // Give the handler time to process.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The listener should still be running — new connections should work.
    let mut stream2 =
        wait_for_socket(sock_path.to_str().unwrap(), Duration::from_secs(5)).await;
    send_event(&mut stream2, &make_started_event(1)).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let builds = state.list_builds().await;
    assert_eq!(builds.len(), 1);
}

#[tokio::test]
async fn clean_disconnect_does_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let sock_path = dir.path().join("events.sock");
    let sock_str = sock_path.to_str().unwrap().to_string();

    let state = SharedState::new();
    let listener_state = state.clone();

    let _handle = tokio::spawn(async move {
        event_listener::run(&sock_str, listener_state).await.ok();
    });

    let stream = wait_for_socket(sock_path.to_str().unwrap(), Duration::from_secs(5)).await;
    // Just drop the connection immediately.
    drop(stream);

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Listener should still be running.
    let mut stream2 =
        wait_for_socket(sock_path.to_str().unwrap(), Duration::from_secs(5)).await;
    send_event(&mut stream2, &make_started_event(1)).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(state.list_builds().await.len(), 1);
}
