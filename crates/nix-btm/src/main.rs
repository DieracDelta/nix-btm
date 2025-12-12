use std::{
    collections::HashMap, panic, path::PathBuf, sync::Arc, time::Duration,
};

use clap::Parser;
use futures::{FutureExt, future::join_all};
use mimalloc::MiMalloc;
use nix_btm::{
    app::App,
    cli::Args,
    client_side::client_read_snapshot_into_state,
    double_fork::daemon_double_fork,
    event_loop::event_loop,
    get_stats::get_active_users_and_pids,
    handle_internal_json::{
        BuildJob, JobId, JobsStateInner, handle_daemon_info, setup_unix_socket,
    },
    protocol_common::Update,
    ring_reader::{ReadResult, RingReader},
    ring_writer::RingWriter,
    rpc::{ClientRequest, DaemonResponse},
    rpc_client::send_rpc_request,
    rpc_daemon::handle_rpc_connection,
    shutdown::Shutdown,
    spawn_named,
    tracing_init::init_tracing,
};
use tokio::{
    sync::{RwLock, watch},
    time::interval,
};
use tracing::{error, info};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

pub(crate) fn init_async_runtime() {
    let args = Args::parse();
    if matches!(args, Args::Daemon { daemonize, ..} if daemonize) {
        daemon_double_fork();
    }
    init_tracing(&args);

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("unable to initialize tokio runtime")
        .block_on(
            async move {
                let shutdown = Shutdown::new();
                let shutdown_ = shutdown.clone();
                let fut1 = spawn_named("Root task", async move {
                    match args {
                        Args::Daemon { .. } => {
                            run_daemon(args, shutdown_).await;
                        }
                        Args::Client { .. } => {
                            run_client(args, shutdown_).await;
                        }
                        Args::Standalone {
                            nix_json_file_path, ..
                        } => run_standalone(nix_json_file_path, shutdown_)
                            .await
                            .unwrap(),
                        Args::Debug {
                            nix_json_file_path,
                            dump_interval,
                        } => run_debug(
                            nix_json_file_path,
                            dump_interval,
                            shutdown_,
                        )
                        .await
                        .unwrap(),
                    }
                });
                let fut2 = async move {
                    let ctrl_c = tokio::signal::ctrl_c();
                    ctrl_c.await.expect("Failed to listen for Ctrl-C");
                    eprintln!("\nReceived Ctrl-C, shutting down...");
                    shutdown.trigger();
                };
                tokio::select!(
                    _res = fut1 => { },
                    _res2 = fut2 => { }

                );
            }
            .boxed(),
        );
}

// MUST call this with daemon variant
#[allow(dead_code)]
pub(crate) async fn run_daemon(args: Args, shutdown: Shutdown) {
    if let Args::Daemon {
        nix_json_file_path,
        daemon_socket_path,
        ..
    } = args
    {
        let nix_socket_path = nix_json_file_path
            .map_or_else(|| PathBuf::from("/tmp/nixbtm.sock"), PathBuf::from);
        let rpc_socket_path = PathBuf::from(&daemon_socket_path);

        error!("Starting nix-btm daemon");
        error!("  Nix log socket: {}", nix_socket_path.display());
        error!("  RPC socket: {}", rpc_socket_path.display());

        // Create the shared state
        let state: Arc<RwLock<JobsStateInner>> =
            Arc::new(RwLock::new(JobsStateInner::default()));

        // Create watch channel for internal state updates
        let (state_tx, mut state_rx) =
            watch::channel(JobsStateInner::default());

        // Create the ring buffer writer with unique name per daemon instance
        let daemon_pid = std::process::id();
        let shm_name = format!("nix-btm-ring-{daemon_pid}");
        let ring_size: u32 = 1024 * 1024; // 1MB
        let ring_writer = match RingWriter::create(&shm_name, ring_size) {
            Ok(w) => w,
            Err(e) => {
                error!("Failed to create ring buffer: {e}");
                return;
            }
        };
        let ring_writer = Arc::new(RwLock::new(ring_writer));
        error!("Ring buffer created: {shm_name} ({ring_size} bytes)");

        // Set up the RPC socket for client connections
        let (rpc_listener, _rpc_guard) =
            match setup_unix_socket(&rpc_socket_path, 0o666) {
                Ok(l) => l,
                Err(e) => {
                    error!("Failed to set up RPC socket: {e}");
                    return;
                }
            };
        info!("RPC socket listening on {}", rpc_socket_path.display());

        // Spawn RPC handler task
        let rpc_ring_writer = ring_writer.clone();
        let rpc_state = state.clone();
        let rpc_shutdown = shutdown.clone();
        spawn_named("rpc-listener", async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(100));
            ticker.set_missed_tick_behavior(
                tokio::time::MissedTickBehavior::Skip,
            );
            let rpc_shutdown_fut = rpc_shutdown.wait();
            tokio::pin!(rpc_shutdown_fut);
            loop {
                tokio::select! {
                    biased;
                    res = rpc_listener.accept() => {
                        match res {
                            Ok((stream, _addr)) => {
                                let writer = rpc_ring_writer.clone();
                                let st = rpc_state.clone();

                                spawn_named("rpc-connection", async move {
                                    let res = handle_rpc_connection(
                                        stream, writer, st,
                                    )
                                    .await;
                                    if let Err(e) = res {
                                        error!("RPC connection error: {e}");
                                    }
                                });
                            }
                            Err(e) => {
                                error!("RPC accept error: {e}");
                            }
                        }
                    }
                    () = &mut rpc_shutdown_fut => {
                        error!("rpc listener shut down!");
                        return;
                    }
                }
            }
        });

        // Spawn the update streamer task - writes updates to ring buffer
        let streamer_ring_writer = ring_writer.clone();
        let mut streamer_state_rx = state_rx.clone();
        let streamer_shutdown = shutdown.clone();
        spawn_named("update-writer-to-ring-buffer", async move {
            use std::collections::{BTreeSet, HashMap};

            use nix_btm::handle_internal_json::Drv;

            let mut last_jobs: HashMap<JobId, BuildJob> = HashMap::new();
            let mut last_dep_nodes: BTreeSet<Drv> = BTreeSet::new();

            let shutdown_fut = streamer_shutdown.wait();
            tokio::pin!(shutdown_fut);

            loop {
                tokio::select!(
                    () = &mut shutdown_fut => {
                        break;
                    }
                    had_change = streamer_state_rx.changed() => {
                        if had_change.is_err() { break; }
                        let current_state = streamer_state_rx.borrow().clone();
                        let mut writer = streamer_ring_writer.write().await;

                        // Find new and updated jobs
                        for (jid, job) in &current_state.jid_to_job {
                            match last_jobs.get(jid) {
                                None => {
                                    // New job
                                    let update = Update::JobNew(job.clone());
                                    if let Err(e) = writer.write_update(&update)
                                    {
                                        error!("Failed to write JobNew: {e}");
                                    }
                                }
                                Some(old_job) if old_job != job => {
                                    // Updated job - send status change
                                    let update = Update::JobUpdate {
                                        jid: jid.0,
                                        status: format!("{:?}", job.status),
                                    };
                                    if let Err(e) = writer.write_update(&update)
                                    {
                                        error!("Failed to write JobUpdate: {e}");
                                    }
                                }
                                _ => {}
                            }
                        }

                        // Check for finished jobs
                        for (jid, job) in &current_state.jid_to_job {
                            if let Some(stop_time) = job.stop_time_ns
                                && last_jobs
                                    .get(jid)
                                    .is_some_and(|j| j.stop_time_ns.is_none())
                            {
                                let update = Update::JobFinish {
                                    jid: jid.0,
                                    stop_time_ns: stop_time,
                                };
                                if let Err(e) = writer.write_update(&update) {
                                    error!("Failed to write JobFinish: {e}");
                                }
                            }
                        }

                        // Send dep graph updates for new nodes
                        for (drv, node) in &current_state.dep_tree.nodes {
                            if !last_dep_nodes.contains(drv) {
                                let update = Update::DepGraphUpdate {
                                    drv: drv.clone(),
                                    deps: node.deps.iter().cloned().collect(),
                                };
                                if let Err(e) = writer.write_update(&update) {
                                    error!("Failed to write DepGraphUpdate: {e}");
                                }
                            }
                        }

                        last_jobs.clone_from(&current_state.jid_to_job);
                        last_dep_nodes =
                            current_state.dep_tree.nodes.keys().cloned()
                                .collect();

                    }

                );
            }
        });

        // Spawn task to sync watch channel updates to shared state
        let sync_state = state.clone();
        spawn_named("state-sync", async move {
            loop {
                if state_rx.changed().await.is_err() {
                    // should naturally finish when other task finishes
                    break;
                }
                let new_state = state_rx.borrow().clone();
                *sync_state.write().await = new_state;
            }
        });

        // Run the main Nix log handler (this takes over the current task)
        handle_daemon_info(nix_socket_path, 0o666, shutdown.clone(), state_tx)
            .await;

        info!("Daemon shutting down");
    } else {
        unreachable!();
    }
}

#[allow(dead_code)]
pub(crate) async fn run_client(args: Args, is_shutdown: Shutdown) {
    if let Args::Client {
        daemon_socket_path,
        client_log_path: _,
    } = args
    {
        let rpc_socket_path = daemon_socket_path
            .unwrap_or_else(|| "/tmp/nix-daemon.sock".to_string());

        info!("Connecting to daemon at {}", rpc_socket_path);

        // Connect to daemon RPC socket
        let mut stream = match tokio::net::UnixStream::connect(&rpc_socket_path)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to connect to daemon: {e}");
                error!("Is the daemon running? Start it with: nix-btm daemon");
                return;
            }
        };

        // Request ring buffer info
        let ring_response =
            match send_rpc_request(&mut stream, ClientRequest::RequestRing)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    error!("Failed to request ring buffer: {e}");
                    return;
                }
            };

        let (ring_name, ring_total_len) = match ring_response {
            DaemonResponse::RingReady {
                ring_name,
                total_len,
            } => (ring_name, total_len),
            DaemonResponse::Error { message } => {
                error!("Daemon error: {message}");
                return;
            }
            res @ DaemonResponse::SnapshotReady { .. } => {
                error!("Unexpected ring response from daemon {res:?}");
                return;
            }
        };

        info!("Ring buffer: {} ({} bytes)", ring_name, ring_total_len);

        // Request snapshot for initial state
        let client_pid = std::process::id() as i32;
        let snapshot_response = match send_rpc_request(
            &mut stream,
            ClientRequest::RequestSnapshot { client_pid },
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                error!("Failed to request snapshot: {e}");
                return;
            }
        };

        let (snapshot_name, snapshot_len, snap_seq) = match snapshot_response {
            DaemonResponse::SnapshotReady {
                snapshot_name,
                total_len,
                snap_seq,
            } => (snapshot_name, total_len, snap_seq),
            DaemonResponse::Error { message } => {
                error!("Daemon error: {message}");
                return;
            }
            res @ DaemonResponse::RingReady { .. } => {
                error!("Unexpected response from daemon {res:?}");
                return;
            }
        };

        // Read initial state from snapshot
        let initial_state =
            match client_read_snapshot_into_state(&snapshot_name, snapshot_len)
            {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to read snapshot: {e}");
                    return;
                }
            };

        info!(
            "Loaded initial state with {} jobs",
            initial_state.jid_to_job.len()
        );

        // Create ring reader
        let mut ring_reader =
            match RingReader::from_name(&ring_name, ring_total_len as usize) {
                Ok(mut r) => {
                    // Sync to start reading after the snapshot sequence
                    r.sync_to_snapshot(snap_seq);
                    r
                }
                Err(e) => {
                    error!("Failed to create ring reader: {e}");
                    return;
                }
            };

        // Create watch channel for job updates
        let (tx_jobs, recv_job_updates) = watch::channel(initial_state);

        // Spawn ring buffer reader in blocking thread pool since it uses
        // blocking futex/kqueue waits
        let ring_shutdown = is_shutdown.clone();
        let tx_jobs_clone = tx_jobs.clone();
        tokio::task::spawn_blocking(move || {
            loop {
                if ring_shutdown.is_shutdown() {
                    break;
                }

                match ring_reader.try_read() {
                    ReadResult::Update { seq: _, update } => {
                        // Apply update to state
                        tx_jobs_clone.send_modify(|state| {
                            apply_update(state, update);
                        });
                    }
                    ReadResult::Lost { from, to } => {
                        error!("Lost updates from seq {} to {}", from, to);
                        // TODO: Request new snapshot
                    }
                    ReadResult::NeedCatchup => {
                        error!("Need catchup - ring buffer overrun");
                        // TODO: Request new snapshot
                    }
                    ReadResult::NoUpdate => {
                        // No update available, wait for notification
                        if ring_reader.has_waiter() {
                            // Use efficient futex/kqueue wait
                            if let Err(e) = ring_reader.wait_for_update() {
                                error!("Wait error: {e}");
                                std::thread::sleep(Duration::from_millis(10));
                            }
                        } else {
                            // Fallback to polling
                            std::thread::sleep(Duration::from_millis(10));
                        }
                    }
                }
            }
        });

        // Spawn process stats poller
        let (tx, recv_proc_updates) = watch::channel(HashMap::default());
        let shutdown__ = is_shutdown.clone();
        spawn_named("proc info handler", async move {
            let mut interval = interval(Duration::from_secs(1));

            let shutdown_fut = shutdown__.wait();
            tokio::pin!(shutdown_fut);
            loop {
                tokio::select! {
                    () = &mut shutdown_fut => {
                        error!("proc info task returned (shutdown)!");
                        break;
                    }
                    _ = interval.tick() => {
                        let user_map_new = get_active_users_and_pids();
                        if tx.send(user_map_new).is_err() {
                            error!("proc info task: receiver dropped, stopping");
                            break;
                        }
                    }
                }
            }
        });

        // Create app and run TUI
        let app = Box::new(App::default());

        let main_app_handle = spawn_named("tui-drawer", async move {
            event_loop(app, is_shutdown, recv_proc_updates, recv_job_updates)
                .await;
        });

        main_app_handle.await.ok();

        info!("Client shutting down");
    } else {
        unreachable!();
    }
}

/// Apply an update from the ring buffer to the state
fn apply_update(state: &mut JobsStateInner, update: Update) {
    match update {
        Update::JobNew(job) => {
            let drv = job.drv.clone();
            let jid = job.jid;
            state.jid_to_job.insert(jid, job);
            state.drv_to_jobs.entry(drv).or_default().insert(jid);
            state.increment_version();
        }
        Update::JobUpdate { jid, status } => {
            if let Some(job) = state.jid_to_job.get_mut(&jid.into()) {
                // Parse status string back to JobStatus
                // For now, just update as BuildPhaseType
                use nix_btm::handle_internal_json::JobStatus;
                job.status = JobStatus::BuildPhaseType(status);
                state.increment_version();
            }
        }
        Update::JobFinish { jid, stop_time_ns } => {
            if let Some(job) = state.jid_to_job.get_mut(&jid.into()) {
                job.stop_time_ns = Some(stop_time_ns);
                job.status = job.status.mark_complete();
                state.increment_version();
            }
        }
        Update::DepGraphUpdate { drv, deps } => {
            use std::collections::BTreeSet;

            use nix_btm::derivation_tree::DrvNode;

            // Create node with dependencies
            let node = DrvNode {
                root: drv.clone(),
                deps: deps.iter().cloned().collect::<BTreeSet<_>>(),
                required_outputs: BTreeSet::new(), /* Outputs not tracked via
                                                    * Update protocol yet */
                required_output_paths: BTreeSet::new(),
            };

            // Insert node into tree
            state.dep_tree.nodes.insert(drv, node.clone());

            // Use insert_node which properly handles tree_roots
            // by checking if this node is a child of existing roots
            state.dep_tree.insert_node(node);
            state.increment_version();
        }
        Update::Heartbeat { daemon_seq: _ } => {
            // Heartbeat received, daemon is alive
            // No state change, no version increment
        }
    }
}

pub fn main() {
    //assert!(sysinfo::IS_SUPPORTED_SYSTEM, "This OS is supported!");

    init_async_runtime();

    //let sets = get_active_users_and_pids();
    //let mut total_set = HashSet::new();
    //for (_, set) in sets {
    //    let sett: HashSet<_> = set.into_iter().collect();
    //    let unioned = total_set.union(&sett).cloned();
    //    total_set = unioned.collect::<HashSet<_>>();
    //}
    //let mut map = construct_pid_map(total_set.clone());
    //let total_tree = construct_tree(map.keys().cloned().collect(), &mut map)
    //    .into_iter()
    //    .next()
    //    .unwrap()
    //    .1;
    //let real_roots = strip_tf_outta_tree(total_tree, &map);
    //let drvs_roots = get_drvs(real_roots);
    //println!("{:#?}", drvs_roots);
    // dump_pids(&real_roots, &map);
    // println!("{t:#?}");

    // construct_everything();
}

#[allow(dead_code)]
async fn run_standalone(
    socket: Option<String>,
    shutdown: Shutdown,
) -> nix_btm::app::Result<()> {
    let shutdown_ = shutdown.clone();
    let shutdown__ = shutdown.clone();

    let (tx_jobs, recv_job_updates): (_, watch::Receiver<JobsStateInner>) =
        watch::channel(JobsStateInner::default());
    let maybe_jh = socket.map(|socket| {
        spawn_named("listening for new connections", async move {
            handle_daemon_info(socket.into(), 0o660, shutdown_, tx_jobs).await;
            error!("main loop returned!");
        })
    });

    // create app and run it
    let app = Box::new(App::default());

    let (tx, recv_proc_updates) = watch::channel(HashMap::default());
    let t_handle = spawn_named("proc info handler", async move {
        let mut interval = interval(Duration::from_secs(1));

        let shutdown_fut = shutdown__.wait();
        tokio::pin!(shutdown_fut);
        loop {
            tokio::select! {
                () = &mut shutdown_fut => {
                    error!("proc info task returned (shutdown)!");
                    break;
                }
                _ = interval.tick() => {
                    let user_map_new = get_active_users_and_pids();
                    if tx.send(user_map_new).is_err() {
                        error!("proc info task: receiver dropped, stopping");
                        break;
                    }
                }
            }
        }
    });

    let main_app_handle = spawn_named("tui drawer", async move {
        event_loop(app, shutdown, recv_proc_updates, recv_job_updates).await;
        error!("main loop returned!");
    });

    let mut handles = vec![t_handle, main_app_handle];

    if let Some(jh) = maybe_jh {
        handles.push(jh);
    }

    error!("waiting for all handles to return");
    join_all(handles).await;

    Ok(())
}

/// Debug mode: listens on socket and dumps state to stdout periodically
pub(crate) async fn run_debug(
    nix_json_file_path: Option<String>,
    dump_interval: u64,
    shutdown: Shutdown,
) -> nix_btm::app::Result<()> {
    let shutdown_ = shutdown.clone();
    let socket = nix_json_file_path.map(PathBuf::from);

    let (tx_jobs, mut recv_job_updates): (_, watch::Receiver<JobsStateInner>) =
        watch::channel(JobsStateInner::default());

    // Start listening on socket if provided
    let maybe_jh = socket.map(|socket| {
        spawn_named("listening for new connections", async move {
            handle_daemon_info(socket, 0o660, shutdown_, tx_jobs).await;
            eprintln!("[DEBUG] Socket listener exited");
        })
    });

    // Periodically dump state
    let dump_handle = spawn_named("state dumper", async move {
        let mut interval = interval(Duration::from_secs(dump_interval));
        let shutdown_fut = shutdown.wait();
        tokio::pin!(shutdown_fut);

        eprintln!("=== NIX-BTM DEBUG MODE ===");
        eprintln!(
            "Listening on socket, will dump state every {dump_interval} \
             seconds"
        );
        eprintln!("Press Ctrl+C to exit\n");

        loop {
            tokio::select! {
                () = &mut shutdown_fut => {
                    eprintln!("[DEBUG] Dumper shutting down");
                    break;
                }
                _ = interval.tick() => {
                    // Wait for state update
                    if recv_job_updates.changed().await.is_ok() {
                        let state = recv_job_updates.borrow().clone();
                        dump_state(&state);
                    }
                }
            }
        }
    });

    let mut handles = vec![dump_handle];
    if let Some(jh) = maybe_jh {
        handles.push(jh);
    }

    join_all(handles).await;
    Ok(())
}

fn dump_state(state: &JobsStateInner) {
    use nix_btm::tree_generation::{
        PruneType, TreeCache, gen_drv_tree_leaves_from_state,
    };

    println!("\n{:=<80}", "");
    println!("TIMESTAMP: {:?}", std::time::SystemTime::now());
    println!("{:=<80}\n", "");

    // Dump targets
    println!("TARGETS ({}):", state.targets.len());
    for (id, target) in &state.targets {
        println!(
            "  [{:?}] {} (requester: {:?})",
            id, target.reference, target.requester_id
        );
        println!("    Status: {:?}", target.status);
        println!("    Root drv: {}", target.root_drv.name);
        println!(
            "    Transitive closure: {} drvs",
            target.transitive_closure.len()
        );
    }
    println!();

    // Dump jobs
    println!("JOBS ({}):", state.jid_to_job.len());
    for (jid, job) in &state.jid_to_job {
        println!(
            "  [JobId({:?})] {} - {:?} (requester: {:?})",
            jid.0, job.drv.name, job.status, job.rid
        );
    }
    println!();

    // Dump dep tree
    println!("DEP TREE:");
    println!("  Nodes: {}", state.dep_tree.nodes.len());
    println!("  Tree roots: {}", state.dep_tree.tree_roots.len());
    for root in &state.dep_tree.tree_roots {
        println!("    - {}", root.name);
    }
    println!();

    // Dump drv_to_targets mapping
    println!("DRV_TO_TARGETS ({} entries):", state.drv_to_targets.len());
    for (drv, targets) in state.drv_to_targets.iter().take(10) {
        println!("  {} -> {:?}", drv.name, targets);
    }
    if state.drv_to_targets.len() > 10 {
        println!("  ... ({} more)", state.drv_to_targets.len() - 10);
    }
    println!();

    // Generate and dump tree for each prune mode
    for prune_mode in
        [PruneType::None, PruneType::Normal, PruneType::Aggressive]
    {
        println!("TREE (PruneType::{prune_mode:?}):");
        let mut cache = TreeCache::default();
        let tree =
            gen_drv_tree_leaves_from_state(&mut cache, state, prune_mode);
        if tree.is_empty() {
            println!("  (empty)");
        } else {
            for (i, root) in tree.iter().enumerate() {
                println!(
                    "  [{}] {} (children: {})",
                    i,
                    root.identifier(),
                    root.children().len()
                );
                // Show first level of children
                for child in root.children().iter().take(5) {
                    println!(
                        "    - {} (children: {})",
                        child.identifier(),
                        child.children().len()
                    );
                }
                if root.children().len() > 5 {
                    println!("    ... ({} more)", root.children().len() - 5);
                }
            }
        }
        println!();
    }

    println!("{:=<80}\n", "");
}
