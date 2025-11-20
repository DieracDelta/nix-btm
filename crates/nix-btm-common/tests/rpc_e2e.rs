use std::sync::Arc;
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;

use nix_btm_common::{
    handle_internal_json::JobsStateInner,
    ring_writer::RingWriter,
    rpc::{ClientRequest, DaemonResponse},
    rpc_client::send_rpc_request,
    rpc_daemon::handle_rpc_connection,
    ring_reader::RingReader,
};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::RwLock;

/// Helper to create a temporary Unix socket listener with unique path
async fn create_test_listener(test_name: &str) -> (UnixListener, String) {
    let socket_path = format!("/tmp/nix-btm-rpc-test-{}.sock", test_name);

    // Remove socket if it exists
    let _ = std::fs::remove_file(&socket_path);

    let listener = UnixListener::bind(&socket_path)
        .expect("Failed to bind Unix socket");

    (listener, socket_path)
}

#[tokio::test]
async fn test_rpc_request_ring_fd() {
    // Cleanup any leftover shm from previous test runs
    let _ = std::fs::remove_file("/dev/shm/test_rpc_ring");

    // Setup: Create ring buffer
    let ring_len = 1024u32;
    let writer = RingWriter::create("test_rpc_ring", ring_len)
        .expect("Failed to create ring writer");
    let writer = Arc::new(RwLock::new(writer));

    // Create empty state
    let state = Arc::new(RwLock::new(JobsStateInner::default()));

    // Setup: Create Unix socket listener
    let (listener, socket_path) = create_test_listener("ring_fd").await;

    // Spawn daemon handler
    let writer_clone = writer.clone();
    let state_clone = state.clone();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let _ = handle_rpc_connection(stream, writer_clone, state_clone).await;
        }
    });

    // Give the listener time to start
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Client: Connect and request ring
    let mut client = UnixStream::connect(&socket_path).await
        .expect("Failed to connect to daemon");

    let request = ClientRequest::RequestRing;
    let response = send_rpc_request(&mut client, request).await
        .expect("RPC request failed");

    // Verify response
    match response {
        DaemonResponse::RingReady { ring_name, total_len } => {
            println!("Received RingReady: name={}, total_len={}", ring_name, total_len);

            // Verify we can create a RingReader from the shared memory name
            let reader = RingReader::from_name(&ring_name, total_len as usize)
                .expect("Failed to create RingReader from shared memory name");

            // Verify the reader has the correct ring_len
            assert_eq!(reader.ring_len, ring_len);

            println!("Successfully created RingReader from shared memory");
        }
        other => panic!("Expected RingReady, got {:?}", other),
    }

    // Cleanup
    let _ = std::fs::remove_file(&socket_path);
}

#[tokio::test]
async fn test_rpc_request_snapshot() {
    // Use a unique PID for this test to avoid collisions with parallel tests
    let mut hasher = DefaultHasher::new();
    std::thread::current().id().hash(&mut hasher);
    let client_pid = (hasher.finish() % 1000000) as i32 + 1000000;

    // Cleanup any leftover shm from previous test runs
    let _ = std::fs::remove_file("/dev/shm/test_rpc_snapshot");
    let _ = std::fs::remove_file(&format!("/dev/shm/nix-btm-snapshot-p{}", client_pid));

    // Setup: Create ring buffer
    let ring_len = 1024u32;
    let writer = RingWriter::create("test_rpc_snapshot", ring_len)
        .expect("Failed to create ring writer");
    let writer = Arc::new(RwLock::new(writer));

    // Create state with some data
    let state = Arc::new(RwLock::new(JobsStateInner::default()));

    // Setup: Create Unix socket listener
    let (listener, socket_path) = create_test_listener("snapshot").await;

    // Spawn daemon handler
    let writer_clone = writer.clone();
    let state_clone = state.clone();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let _ = handle_rpc_connection(stream, writer_clone, state_clone).await;
        }
    });

    // Give the listener time to start
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Client: Connect and request snapshot
    let mut client = UnixStream::connect(&socket_path).await
        .expect("Failed to connect to daemon");

    let request = ClientRequest::RequestSnapshot { client_pid };
    let response = send_rpc_request(&mut client, request).await
        .expect("RPC request failed");

    // Verify response
    match response {
        DaemonResponse::SnapshotReady {
            snapshot_name,
            total_len,
            snap_seq,
        } => {
            println!("Received SnapshotReady:");
            println!("  snapshot_name: {}", snapshot_name);
            println!("  total_len: {}", total_len);
            println!("  snap_seq: {}", snap_seq);

            // Verify snapshot name matches expected format
            let expected_name = format!("nix-btm-snapshot-p{}", client_pid);
            assert_eq!(snapshot_name, expected_name);

            // Try to read the snapshot
            let state = nix_btm_common::client_side::client_read_snapshot_into_state(
                &snapshot_name,
                total_len,
            ).expect("Failed to read snapshot");

            println!("Successfully read snapshot with {} jobs", state.jid_to_job.len());
        }
        DaemonResponse::Error { message } => {
            panic!("Got error response: {}", message);
        }
        other => panic!("Expected SnapshotReady, got {:?}", other),
    }

    // Cleanup
    let _ = std::fs::remove_file(&socket_path);
}

#[tokio::test]
async fn test_rpc_multiple_requests() {
    // Use a unique PID for this test to avoid collisions with parallel tests
    let mut hasher = DefaultHasher::new();
    std::thread::current().id().hash(&mut hasher);
    let client_pid = (hasher.finish() % 1000000) as i32 + 2000000;

    // Cleanup any leftover shm from previous test runs
    let _ = std::fs::remove_file("/dev/shm/test_rpc_multi");
    let _ = std::fs::remove_file(&format!("/dev/shm/nix-btm-snapshot-p{}", client_pid));

    // Setup: Create ring buffer
    let ring_len = 1024u32;
    let writer = RingWriter::create("test_rpc_multi", ring_len)
        .expect("Failed to create ring writer");
    let writer = Arc::new(RwLock::new(writer));

    let state = Arc::new(RwLock::new(JobsStateInner::default()));

    // Setup: Create Unix socket listener
    let (listener, socket_path) = create_test_listener("multi").await;

    // Spawn daemon handler
    let writer_clone = writer.clone();
    let state_clone = state.clone();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let _ = handle_rpc_connection(stream, writer_clone, state_clone).await;
        }
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Client: Connect once
    let mut client = UnixStream::connect(&socket_path).await
        .expect("Failed to connect to daemon");

    // Request 1: Ring
    let response1 = send_rpc_request(&mut client, ClientRequest::RequestRing).await
        .expect("First request failed");

    assert!(matches!(response1, DaemonResponse::RingReady { .. }));
    println!("Request 1 (Ring): Success");

    // Request 2: Snapshot
    let client_pid = std::process::id() as i32;
    let response2 = send_rpc_request(
        &mut client,
        ClientRequest::RequestSnapshot { client_pid }
    ).await.expect("Second request failed");

    if !matches!(response2, DaemonResponse::SnapshotReady { .. }) {
        eprintln!("ERROR: Expected SnapshotReady, got: {:?}", response2);
    }
    assert!(matches!(response2, DaemonResponse::SnapshotReady { .. }));
    println!("Request 2 (Snapshot): Success");

    // Cleanup
    let _ = std::fs::remove_file(&socket_path);
}
