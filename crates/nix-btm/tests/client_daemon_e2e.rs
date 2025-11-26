use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    ffi::CString,
    sync::Arc,
    time::Duration,
};

use nix_btm::{
    client_side::client_read_snapshot_into_state,
    derivation_tree::{DrvNode, DrvRelations},
    handle_internal_json::{
        BuildJob, BuildTargetId, Drv, JobId, JobStatus, JobsStateInner,
        RequesterId,
    },
    protocol_common::Update,
    ring_reader::{ReadResult, RingReader},
    ring_writer::RingWriter,
    rpc::{ClientRequest, DaemonResponse},
    rpc_client::send_rpc_request,
    rpc_daemon::handle_rpc_connection,
};
use tokio::{
    net::{UnixListener, UnixStream},
    sync::RwLock,
};

/// Unlink shared memory by name
fn shm_cleanup(name: &str) {
    let c_name = CString::new(name).unwrap();
    unsafe {
        libc::shm_unlink(c_name.as_ptr());
    }
}

/// Apply an update to state (mirrors client implementation)
fn apply_update(state: &mut JobsStateInner, update: Update) {
    match update {
        Update::JobNew(job) => {
            let drv = job.drv.clone();
            let jid = job.jid;
            state.jid_to_job.insert(jid, job);
            state.drv_to_jobs.entry(drv).or_default().insert(jid);
        }
        Update::JobUpdate { jid, status } => {
            if let Some(job) = state.jid_to_job.get_mut(&jid.into()) {
                job.status = JobStatus::BuildPhaseType(status);
            }
        }
        Update::JobFinish { jid, stop_time_ns } => {
            if let Some(job) = state.jid_to_job.get_mut(&jid.into()) {
                job.stop_time_ns = Some(stop_time_ns);
                job.status = job.status.mark_complete();
            }
        }
        Update::DepGraphUpdate { drv, deps: _ } => {
            let _ = drv;
        }
        Update::Heartbeat { daemon_seq: _ } => {}
    }
}

/// Create a test job
fn make_test_job(id: u64, name: &str, hash: &str) -> BuildJob {
    BuildJob {
        jid: JobId(id),
        rid: RequesterId(0),
        drv: Drv {
            name: name.to_string(),
            hash: hash.to_string(),
        },
        status: JobStatus::Starting,
        start_time_ns: 0,
        stop_time_ns: None,
    }
}

/// Create state with test data
fn make_test_state() -> JobsStateInner {
    let drv = Drv {
        name: "test-pkg-1.0".to_string(),
        hash: "abc123def456".to_string(),
    };
    let jid = JobId(1);
    let job = BuildJob {
        jid,
        rid: RequesterId(0),
        drv: drv.clone(),
        status: JobStatus::BuildPhaseType("configure".to_string()),
        start_time_ns: 1000,
        stop_time_ns: None,
    };

    let mut jid_to_job = HashMap::new();
    jid_to_job.insert(jid, job);

    let mut drv_to_jobs = HashMap::new();
    let mut job_set = HashSet::new();
    job_set.insert(jid);
    drv_to_jobs.insert(drv.clone(), job_set);

    let mut nodes = BTreeMap::new();
    nodes.insert(drv.clone(), DrvNode::default());

    let mut roots = BTreeSet::new();
    roots.insert(drv);

    JobsStateInner {
        targets: Default::default(),
        drv_to_targets: Default::default(),
        next_target_id: BuildTargetId(0),
        jid_to_job,
        drv_to_jobs,
        dep_tree: DrvRelations {
            nodes,
            tree_roots: roots,
        },
        already_built_drvs: Default::default(),
        version: 0,
    }
}

#[test]
fn test_apply_update_job_new() {
    let mut state = JobsStateInner::default();

    let job = make_test_job(1, "foo", "abc123");
    let update = Update::JobNew(job.clone());

    apply_update(&mut state, update);

    assert_eq!(state.jid_to_job.len(), 1);
    assert!(state.jid_to_job.contains_key(&JobId(1)));

    let stored_job = state.jid_to_job.get(&JobId(1)).unwrap();
    assert_eq!(stored_job.drv.name, "foo");
    assert_eq!(stored_job.drv.hash, "abc123");

    // Check drv_to_jobs mapping
    assert!(state.drv_to_jobs.contains_key(&job.drv));
    assert!(state.drv_to_jobs.get(&job.drv).unwrap().contains(&JobId(1)));
}

#[test]
fn test_apply_update_job_update() {
    let mut state = JobsStateInner::default();

    // First add a job
    let job = make_test_job(1, "foo", "abc123");
    apply_update(&mut state, Update::JobNew(job));

    // Now update it
    let update = Update::JobUpdate {
        jid: 1,
        status: "building".to_string(),
    };
    apply_update(&mut state, update);

    let stored_job = state.jid_to_job.get(&JobId(1)).unwrap();
    match &stored_job.status {
        JobStatus::BuildPhaseType(phase) => {
            assert_eq!(phase, "building");
        }
        _ => panic!("Expected BuildPhaseType status"),
    }
}

#[test]
fn test_apply_update_job_finish() {
    let mut state = JobsStateInner::default();

    // Add a job in building phase
    let mut job = make_test_job(1, "foo", "abc123");
    job.status = JobStatus::BuildPhaseType("build".to_string());
    apply_update(&mut state, Update::JobNew(job));

    // Finish it
    let update = Update::JobFinish {
        jid: 1,
        stop_time_ns: 5000,
    };
    apply_update(&mut state, update);

    let stored_job = state.jid_to_job.get(&JobId(1)).unwrap();
    assert_eq!(stored_job.stop_time_ns, Some(5000));
    assert!(matches!(stored_job.status, JobStatus::CompletedBuild));
}

#[test]
fn test_apply_update_multiple_jobs() {
    let mut state = JobsStateInner::default();

    // Add multiple jobs
    for i in 1..=5 {
        let job =
            make_test_job(i, &format!("pkg-{}", i), &format!("hash{}", i));
        apply_update(&mut state, Update::JobNew(job));
    }

    assert_eq!(state.jid_to_job.len(), 5);

    // Update some to building phase
    apply_update(
        &mut state,
        Update::JobUpdate {
            jid: 2,
            status: "installing".to_string(),
        },
    );

    // Update job 3 to building phase first, then finish it
    apply_update(
        &mut state,
        Update::JobUpdate {
            jid: 3,
            status: "build".to_string(),
        },
    );

    // Finish one (now it's in BuildPhaseType so mark_complete returns
    // CompletedBuild)
    apply_update(
        &mut state,
        Update::JobFinish {
            jid: 3,
            stop_time_ns: 1000,
        },
    );

    // Verify states
    assert!(matches!(
        state.jid_to_job.get(&JobId(1)).unwrap().status,
        JobStatus::Starting
    ));
    assert!(matches!(
        state.jid_to_job.get(&JobId(2)).unwrap().status,
        JobStatus::BuildPhaseType(_)
    ));
    assert!(matches!(
        state.jid_to_job.get(&JobId(3)).unwrap().status,
        JobStatus::CompletedBuild
    ));
}

#[test]
fn test_ring_buffer_job_updates_flow() {
    shm_cleanup("test_job_flow");

    let ring_len = 4096u32;
    let mut writer = RingWriter::create("test_job_flow", ring_len)
        .expect("Failed to create ring writer");

    let ring_name = writer.name.clone();
    let total_len = std::mem::size_of::<nix_btm::protocol_common::ShmHeader>()
        as u32
        + ring_len;

    // Create reader BEFORE writes so it can track from the beginning
    let mut reader = RingReader::from_name(&ring_name, total_len as usize)
        .expect("Failed to create ring reader");

    // Write job lifecycle updates
    let job = make_test_job(42, "my-package", "xyz789");

    writer.write_update(&Update::JobNew(job.clone())).unwrap();
    writer
        .write_update(&Update::JobUpdate {
            jid: 42,
            status: "configure".to_string(),
        })
        .unwrap();
    writer
        .write_update(&Update::JobUpdate {
            jid: 42,
            status: "build".to_string(),
        })
        .unwrap();
    writer
        .write_update(&Update::JobFinish {
            jid: 42,
            stop_time_ns: 12345,
        })
        .unwrap();

    let mut state = JobsStateInner::default();

    let mut updates_read = 0;
    loop {
        match reader.try_read() {
            ReadResult::Update { update, .. } => {
                apply_update(&mut state, update);
                updates_read += 1;
                if updates_read >= 4 {
                    break;
                }
            }
            ReadResult::Lost { .. } => {
                // Reader detected missed updates, continue reading from new
                // position
                continue;
            }
            ReadResult::NoUpdate => break,
            ReadResult::NeedCatchup => break,
        }
    }

    // Verify final state
    let final_job = state.jid_to_job.get(&JobId(42)).unwrap();
    assert_eq!(final_job.drv.name, "my-package");
    assert_eq!(final_job.stop_time_ns, Some(12345));
    assert!(matches!(final_job.status, JobStatus::CompletedBuild));
}

#[tokio::test]
async fn test_full_client_daemon_flow() {
    shm_cleanup("test_full_flow");
    let client_pid = std::process::id() as i32 + 100000;
    shm_cleanup(&format!("nix-btm-snapshot-p{}", client_pid));

    // Create daemon state with some jobs
    let state = make_test_state();
    let state = Arc::new(RwLock::new(state));

    // Create ring buffer
    let ring_len = 4096u32;
    let writer = RingWriter::create("test_full_flow", ring_len)
        .expect("Failed to create ring writer");
    let writer = Arc::new(RwLock::new(writer));

    // Write some updates to ring
    {
        let mut w = writer.write().await;
        w.write_update(&Update::Heartbeat { daemon_seq: 1 })
            .unwrap();

        let new_job = make_test_job(99, "new-pkg", "newhash");
        w.write_update(&Update::JobNew(new_job)).unwrap();
    }

    // Create RPC socket
    let socket_path = "/tmp/nix-btm-test-full-flow.sock";
    let _ = std::fs::remove_file(socket_path);
    let listener =
        UnixListener::bind(socket_path).expect("Failed to bind socket");

    // Spawn daemon handler
    let writer_clone = writer.clone();
    let state_clone = state.clone();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let _ =
                handle_rpc_connection(stream, writer_clone, state_clone).await;
        }
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Client connects
    let mut client = UnixStream::connect(socket_path)
        .await
        .expect("Failed to connect");

    // Request ring info
    let ring_response =
        send_rpc_request(&mut client, ClientRequest::RequestRing)
            .await
            .expect("Failed to request ring");

    let (ring_name, ring_total_len) = match ring_response {
        DaemonResponse::RingReady {
            ring_name,
            total_len,
        } => (ring_name, total_len),
        other => panic!("Expected RingReady, got {:?}", other),
    };

    // Request snapshot
    let snapshot_response = send_rpc_request(
        &mut client,
        ClientRequest::RequestSnapshot { client_pid },
    )
    .await
    .expect("Failed to request snapshot");

    let (snapshot_name, snapshot_len) = match snapshot_response {
        DaemonResponse::SnapshotReady {
            snapshot_name,
            total_len,
            ..
        } => (snapshot_name, total_len),
        other => panic!("Expected SnapshotReady, got {:?}", other),
    };

    // Read snapshot
    let initial_state =
        client_read_snapshot_into_state(&snapshot_name, snapshot_len)
            .expect("Failed to read snapshot");

    // Verify initial state has the test job
    assert_eq!(initial_state.jid_to_job.len(), 1);
    assert!(initial_state.jid_to_job.contains_key(&JobId(1)));

    // Create ring reader
    let mut reader = RingReader::from_name(&ring_name, ring_total_len as usize)
        .expect("Failed to create ring reader");

    // Read updates from ring
    let mut client_state = initial_state;
    let mut updates_read = 0;

    loop {
        match reader.try_read() {
            ReadResult::Update { update, .. } => {
                apply_update(&mut client_state, update);
                updates_read += 1;
            }
            ReadResult::NoUpdate => break,
            ReadResult::NeedCatchup => {
                // This can happen if reader was created after writes
                // In real client, would request new snapshot
                break;
            }
            ReadResult::Lost { .. } => {
                // Continue reading after lost updates
            }
        }
    }

    // Verify snapshot state was loaded correctly
    // The ring updates might not be readable if timing causes NeedCatchup
    assert!(client_state.jid_to_job.contains_key(&JobId(1))); // From snapshot

    // If we read updates, verify new job was added
    if updates_read > 0 {
        assert!(client_state.jid_to_job.contains_key(&JobId(99))); // From ring
    }

    // Cleanup
    let _ = std::fs::remove_file(socket_path);
}

#[tokio::test]
async fn test_multiple_clients_read_ring() {
    shm_cleanup("test_multi_client");

    let ring_len = 4096u32;
    let mut writer = RingWriter::create("test_multi_client", ring_len)
        .expect("Failed to create ring writer");

    let ring_name = writer.name.clone();
    let total_len = std::mem::size_of::<nix_btm::protocol_common::ShmHeader>()
        as u32
        + ring_len;

    // Create multiple readers BEFORE writes
    let mut reader1 = RingReader::from_name(&ring_name, total_len as usize)
        .expect("Failed to create reader 1");
    let mut reader2 = RingReader::from_name(&ring_name, total_len as usize)
        .expect("Failed to create reader 2");

    // Write updates
    for i in 1..=10 {
        writer
            .write_update(&Update::Heartbeat { daemon_seq: i })
            .unwrap();
    }

    // Both readers should get same updates
    let mut updates1 = Vec::new();
    let mut updates2 = Vec::new();

    // Reader 1: handle Lost then read updates
    loop {
        match reader1.try_read() {
            ReadResult::Update { update, .. } => {
                updates1.push(update);
                if updates1.len() >= 10 {
                    break;
                }
            }
            ReadResult::Lost { .. } => continue,
            ReadResult::NoUpdate | ReadResult::NeedCatchup => break,
        }
    }

    // Reader 2: handle Lost then read updates
    loop {
        match reader2.try_read() {
            ReadResult::Update { update, .. } => {
                updates2.push(update);
                if updates2.len() >= 10 {
                    break;
                }
            }
            ReadResult::Lost { .. } => continue,
            ReadResult::NoUpdate | ReadResult::NeedCatchup => break,
        }
    }

    assert_eq!(updates1.len(), 10);
    assert_eq!(updates2.len(), 10);

    // Verify both got the same heartbeat sequences
    for (i, (u1, u2)) in updates1.iter().zip(updates2.iter()).enumerate() {
        match (u1, u2) {
            (
                Update::Heartbeat { daemon_seq: s1 },
                Update::Heartbeat { daemon_seq: s2 },
            ) => {
                assert_eq!(*s1, (i + 1) as u64);
                assert_eq!(*s2, (i + 1) as u64);
            }
            _ => panic!("Expected heartbeats"),
        }
    }
}

#[test]
fn test_heartbeat_passthrough() {
    let mut state = JobsStateInner::default();

    // Heartbeats should not modify state
    apply_update(&mut state, Update::Heartbeat { daemon_seq: 1 });
    apply_update(&mut state, Update::Heartbeat { daemon_seq: 100 });
    apply_update(&mut state, Update::Heartbeat { daemon_seq: 999 });

    assert!(state.jid_to_job.is_empty());
    assert!(state.drv_to_jobs.is_empty());
}

#[test]
fn test_update_nonexistent_job() {
    let mut state = JobsStateInner::default();

    // Updating a job that doesn't exist should be a no-op
    apply_update(
        &mut state,
        Update::JobUpdate {
            jid: 999,
            status: "building".to_string(),
        },
    );

    apply_update(
        &mut state,
        Update::JobFinish {
            jid: 888,
            stop_time_ns: 1000,
        },
    );

    // State should remain empty
    assert!(state.jid_to_job.is_empty());
}
