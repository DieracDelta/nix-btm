use std::{
    collections::{BTreeSet, HashMap},
    error::Error,
    io::{self, Stdout},
    panic,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use clap::Parser;
use futures::{FutureExt, future::join_all};
use mimalloc::MiMalloc;
use nix_btm::{
    client_side::client_read_snapshot_into_state,
    handle_internal_json::{
        BuildJob, JobId, JobsStateInner, handle_daemon_info, setup_unix_socket,
    },
    protocol_common::Update,
    ring_reader::{ReadResult, RingReader},
    ring_writer::RingWriter,
    rpc::{ClientRequest, DaemonResponse},
    rpc_client::send_rpc_request,
    rpc_daemon::handle_rpc_connection,
    spawn_named,
};
use ratatui::text::Line;
use strum::{Display, EnumCount, EnumIter, FromRepr};
use tokio::sync::RwLock;
use tracing::{error, info, info_span};

pub mod emojis;
pub mod event_loop;
pub mod get_stats;
pub mod gruvbox;
#[cfg(target_os = "linux")]
pub mod listen_to_output;
pub mod tracing_init;
pub mod ui;

use crossterm::{
    event::DisableMouseCapture,
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode,
    },
};
use event_loop::event_loop;
use ratatui::{
    backend::CrosstermBackend, style::Style, widgets::ScrollbarState,
};
use tokio::{runtime::Runtime, sync::watch};
use tui_tree_widget::TreeState;
use ui::{
    BORDER_STYLE_SELECTED, BORDER_STYLE_UNSELECTED, TITLE_STYLE_SELECTED,
    TITLE_STYLE_UNSELECTED,
};

use crate::{
    get_stats::{ProcMetadata, get_active_users_and_pids},
    tracing_init::init_tracing,
    ui::PruneType,
};

static HELP_STR_SOCKET: &str = "
    The fully qualified path of the socket to read from. See the README for \
                                more details. Without this flag, the Eagle \
                                Eye view will not work because it will be \
                                unable to view the nix daemon's state. \
                                Example value: \"/tmp/nixbtm.sock\"
";

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

//static GLOBAL: Jemalloc = Jemalloc;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
type Terminal = ratatui::Terminal<CrosstermBackend<Stdout>>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Pane {
    #[default]
    Left,
    Right,
}

#[derive(
    Default,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Display,
    FromRepr,
    EnumIter,
    EnumCount,
)]
pub enum SelectedTab {
    #[default]
    #[strum(to_string = "Nix Builder View 👷")]
    BuilderView,
    #[strum(to_string = "Eagle Eye View 🦅")]
    EagleEyeView,
    #[strum(to_string = "Build Job View 💼")]
    BuildJobView,
}

impl SelectedTab {
    fn title(self) -> Line<'static> {
        format!("  {self}  ").into()
    }

    fn previous(self) -> Self {
        let current_index: usize = self as usize;
        let previous_index = (current_index + SelectedTab::COUNT)
            .saturating_sub(1)
            % SelectedTab::COUNT;
        Self::from_repr(previous_index).unwrap_or(self)
    }

    fn next(self) -> Self {
        let current_index = self as usize;
        let next_index = current_index.saturating_add(1) % SelectedTab::COUNT;
        Self::from_repr(next_index).unwrap_or(self)
    }
}

#[derive(Default, Debug)]
pub struct App {
    builder_view: BuilderViewState,
    eagle_eye_view: EagleEyeViewState,
    build_job_view: BuildJobViewState,
    tab_selected: SelectedTab,
    // I hate this. Stream updates instead. Better when we separate out to the
    // daemon
    cur_info_builds: JobsStateInner,
    cur_info: HashMap<String, BTreeSet<ProcMetadata>>,
}

#[derive(Default, Copy, Clone, Debug)]
enum TreeToggle {
    Open,
    Closed,
    #[default]
    Never,
}

#[derive(Default, Debug)]
pub struct EagleEyeViewState {
    man_toggle: bool,
    active_only: PruneType,
    state: TreeState<String>,
    perform_toggle: bool,
    last_toggle: TreeToggle,
}

#[derive(Default, Debug)]
pub struct BuildJobViewState {
    man_toggle: bool,
}

#[derive(Default, Debug)]
pub struct BuilderViewState {
    pub vertical_scroll_state: ScrollbarState,
    pub vertical_scroll: usize,
    pub horizontal_scroll: usize,
    state: TreeState<String>,
    pub selected_pane: Pane,
    pub man_toggle: bool,
}

impl BuilderViewState {
    pub fn gen_title_style(&self, this_pane: Pane) -> Style {
        if self.selected_pane == this_pane {
            *TITLE_STYLE_SELECTED
        } else {
            *TITLE_STYLE_UNSELECTED
        }
    }

    pub fn gen_border_style(&self, this_pane: Pane) -> Style {
        if self.selected_pane == this_pane {
            *BORDER_STYLE_SELECTED
        } else {
            *BORDER_STYLE_UNSELECTED
        }
    }

    pub fn go_right(&mut self) {
        if self.selected_pane == Pane::Left {
            self.selected_pane = Pane::Right;
        }
    }

    pub fn go_left(&mut self) {
        if self.selected_pane == Pane::Right {
            self.selected_pane = Pane::Left;
        }
    }
}

use std::process::exit;

use libc::{pid_t, setsid};
use rustix::{
    fs::Mode,
    process::{chdir, umask},
};

// Global flag for signal handler
static SHUTDOWN_SIGNALED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Daemonize (advanced programming in the unix environment)
pub(crate) fn daemon_double_fork() {
    do_fork();

    let sid = unsafe { setsid() };
    if sid < 0 {
        eprintln!("setsid failed");
        exit(-1);
    }

    // cannot be killed by parent
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }

    // really shake them off our tail
    do_fork();

    // no risk of unmounting
    chdir("/").unwrap();

    // clear umask
    umask(Mode::empty());

    // redirect the stds to dev null
    redirect_std_fds_to_devnull();
}

fn redirect_std_fds_to_devnull() {
    use std::{fs::OpenOptions, os::unix::io::AsRawFd};

    let devnull = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")
        .expect("failed to open /dev/null");

    let fd = devnull.as_raw_fd();
    unsafe {
        libc::dup2(fd, 0);
        libc::dup2(fd, 1);
        libc::dup2(fd, 2);
    }
}

fn do_fork() {
    let pid: pid_t = unsafe { libc::fork() };

    match pid {
        p if p < 0 => {
            eprintln!("unable to fork");
            exit(-1);
        }
        0 => {}       // child
        _ => exit(0), // parent
    }
}

pub(crate) fn init_async_runtime() {
    let args = Args::parse();
    if matches!(args, Args::Daemon { daemonize, ..} if daemonize) {
        daemon_double_fork();
    }
    init_tracing(&args);

    let is_shutdown = Arc::new(AtomicBool::new(false));
    let signal_shutdown = is_shutdown.clone();

    let body = async move {
        match args {
            Args::Daemon { .. } => {
                run_daemon(args, is_shutdown).await;
            }
            Args::Client { .. } => {
                run_client(args, is_shutdown).await;
            }
            Args::Standalone {
                nix_json_file_path, ..
            } => run_standalone(nix_json_file_path, is_shutdown)
                .await
                .unwrap(),
        }
    };

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("unable to initialize tokio runtime")
        .block_on(
            async move {
                let fut1 = spawn_named("Root task", body);
                let fut2 = async move {
                    let ctrl_c = tokio::signal::ctrl_c();
                    ctrl_c.await.expect("Failed to listen for Ctrl-C");
                    eprintln!("\nReceived Ctrl-C, shutting down...");
                    signal_shutdown.store(true, Ordering::Relaxed);
                };
                tokio::select!(
                    _res = fut1 => { },
                    _res2 = fut2 => { }

                )
            }
            .boxed(),
        )
}

// MUST call this with daemon variant
#[allow(dead_code)]
pub(crate) async fn run_daemon(args: Args, is_shutdown: Arc<AtomicBool>) {
    if let Args::Daemon {
        nix_json_file_path,
        daemon_socket_path,
        daemon_log_path: _,
        daemonize,
    } = args
    {
        let nix_socket_path = nix_json_file_path
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp/nixbtm.sock"));
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
        let shm_name = format!("nix-btm-ring-{}", daemon_pid);
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
        let rpc_shutdown = is_shutdown.clone();
        spawn_named("rpc-listener", async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(100));
            ticker.set_missed_tick_behavior(
                tokio::time::MissedTickBehavior::Skip,
            );
            loop {
                let accept_fut = rpc_listener.accept();
                tokio::pin!(accept_fut);

                loop {
                    tokio::select! {
                        biased;

                        res = &mut accept_fut => {
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
                            break; // Break inner loop to create new accept future
                        }
                        _ = ticker.tick() => {
                            if rpc_shutdown.load(Ordering::Relaxed) {
                                return;
                            }
                            // Continue inner loop, keeping accept_fut alive
                        }
                    }
                }
            }
        });

        // Spawn the update streamer task - writes updates to ring buffer
        let streamer_ring_writer = ring_writer.clone();
        let mut streamer_state_rx = state_rx.clone();
        let streamer_shutdown = is_shutdown.clone();
        spawn_named("update-streamer", async move {
            use std::collections::{BTreeSet, HashMap};

            use nix_btm::handle_internal_json::Drv;

            let mut last_jobs: HashMap<JobId, BuildJob> = HashMap::new();
            let mut last_dep_nodes: BTreeSet<Drv> = BTreeSet::new();

            loop {
                // Wait for state changes
                if streamer_state_rx.changed().await.is_err() {
                    break;
                }

                if streamer_shutdown.load(Ordering::Relaxed) {
                    break;
                }

                let current_state = streamer_state_rx.borrow().clone();
                let mut writer = streamer_ring_writer.write().await;

                // Find new and updated jobs
                for (jid, job) in &current_state.jid_to_job {
                    match last_jobs.get(jid) {
                        None => {
                            // New job
                            let update = Update::JobNew(job.clone());
                            if let Err(e) = writer.write_update(&update) {
                                error!("Failed to write JobNew: {e}");
                            }
                        }
                        Some(old_job) if old_job != job => {
                            // Updated job - send status change
                            let update = Update::JobUpdate {
                                jid: jid.0,
                                status: format!("{:?}", job.status),
                            };
                            if let Err(e) = writer.write_update(&update) {
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
                            .map(|j| j.stop_time_ns.is_none())
                            .unwrap_or(false)
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

                last_jobs = current_state.jid_to_job.clone();
                last_dep_nodes =
                    current_state.dep_tree.nodes.keys().cloned().collect();
            }
        });

        // Spawn task to sync watch channel updates to shared state
        let sync_state = state.clone();
        spawn_named("state-sync", async move {
            loop {
                if state_rx.changed().await.is_err() {
                    break;
                }
                let new_state = state_rx.borrow().clone();
                *sync_state.write().await = new_state;
            }
        });

        // Run the main Nix log handler (this takes over the current task)
        handle_daemon_info(
            nix_socket_path,
            0o666,
            is_shutdown.clone(),
            state_tx,
        )
        .await;

        info!("Daemon shutting down");
    } else {
        unreachable!();
    }
}

#[allow(dead_code)]
pub(crate) async fn run_client(args: Args, is_shutdown: Arc<AtomicBool>) {
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
            _ => {
                error!("Unexpected response from daemon");
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
            _ => {
                error!("Unexpected response from daemon");
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
                if ring_shutdown.load(Ordering::Relaxed) {
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
        let (tx_proc, recv_proc_updates) = watch::channel(Default::default());
        let proc_shutdown = is_shutdown.clone();
        spawn_named("proc-info-handler", async move {
            while !proc_shutdown.load(Ordering::Relaxed) {
                let user_map_new = get_active_users_and_pids();
                let _ = tx_proc.send(user_map_new);
                tokio::time::sleep(Duration::from_secs(1)).await;
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
        }
        Update::JobUpdate { jid, status } => {
            if let Some(job) = state.jid_to_job.get_mut(&jid.into()) {
                // Parse status string back to JobStatus
                // For now, just update as BuildPhaseType
                use nix_btm::handle_internal_json::JobStatus;
                job.status = JobStatus::BuildPhaseType(status);
            }
        }
        Update::JobFinish { jid, stop_time_ns } => {
            if let Some(job) = state.jid_to_job.get_mut(&jid.into()) {
                job.stop_time_ns = Some(stop_time_ns);
                job.status = job.status.mark_complete();
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
            state.dep_tree.nodes.insert(drv.clone(), node.clone());

            // Use insert_node which properly handles tree_roots
            // by checking if this node is a child of existing roots
            state.dep_tree.insert_node(node);
        }
        Update::Heartbeat { daemon_seq: _ } => {
            // Heartbeat received, daemon is alive
        }
    }
}

pub fn main() {
    if !sysinfo::IS_SUPPORTED_SYSTEM {
        panic!("This OS is supported!");
    }

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

#[derive(clap::Parser)]
#[command(
    name = "nix-btm",
    version,
    about = "nix-btm",
    long_about = "A top-like client for nix that can either run in \
                  conjunction with itself as a corresponding daemon or as a \
                  standalone program with more limited functionality"
)]
enum Args {
    Daemon {
        #[arg(
            long,
            short = 'f',
            value_name = "DAEMONIZE",
            help = "Run in background (double-fork). Example value: false",
            default_value = "false"
        )]
        daemonize: bool,

        #[arg(
            long,
            short,
            value_name = "JSON_FILE_PATH",
            help = HELP_STR_SOCKET,
            default_value = "/tmp/nixbtm.sock"
            )]
        nix_json_file_path: Option<String>,

        #[arg(
            long,
            short,
            value_name = "SOCKET_PATH",
            help = "socket path of daemon",
            default_value = "/tmp/nix-daemon.sock"
        )]
        daemon_socket_path: String,

        #[arg(
            long,
            short = 'l',
            value_name = "LOG_PATH",
            help = "Optional log path value. If not provided, logs will \
                    placed in /tmp/nixbtm-daemon-$PID.log"
        )]
        daemon_log_path: Option<String>,
    },
    Client {
        #[arg(
            long,
            short,
            value_name = "DAEMON_SOCKET_PATH",
            help = HELP_STR_SOCKET,
            default_value = "/tmp/nix-daemon.sock"
        )]
        daemon_socket_path: Option<String>,
        #[arg(
            long,
            short = 'l',
            value_name = "LOG_PATH",
            help = "Optional log path value. If not provided, logs will \
                    placed in /tmp/nixbtm-client-$PID.log"
        )]
        client_log_path: Option<String>,
    },
    Standalone {
        #[arg(
            long,
            short,
            value_name = "JSON_FILE_PATH",
            help = HELP_STR_SOCKET,
            default_value = "/tmp/nixbtm.sock"
            )]
        nix_json_file_path: Option<String>,
        #[arg(
            long,
            short,
            value_name = "LOG_PATH",
            help = "Optional log path value. If not provided, logs will \
                    placed in /tmp/nixbtm-standalone-$PID.log",
            default_value = "None"
        )]
        standalone_log_path: Option<String>,
    },
}

#[allow(dead_code)]
async fn run_standalone(
    socket: Option<String>,
    is_shutdown: Arc<AtomicBool>,
) -> Result<()> {
    let local_is_shutdown = is_shutdown.clone();
    let local_is_shutdown2 = is_shutdown.clone();

    let (tx_jobs, recv_job_updates): (_, watch::Receiver<JobsStateInner>) =
        watch::channel(Default::default());
    let maybe_jh = socket.map(|socket| {
        spawn_named("listening for new connections", async move {
            handle_daemon_info(
                socket.into(),
                0o660,
                local_is_shutdown2,
                tx_jobs,
            )
            .await
        })
    });

    // create app and run it
    let app = Box::new(App::default());

    let (tx, recv_proc_updates) = watch::channel(Default::default());
    let t_handle = spawn_named("proc info handler", async move {
        while !local_is_shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            let user_map_new = get_active_users_and_pids();
            // TODO should do some sort of error checking
            let _ = tx.send(user_map_new);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    let main_app_handle = spawn_named("tui drawer", async move {
        event_loop(app, is_shutdown, recv_proc_updates, recv_job_updates).await;
    });

    let mut handles = vec![t_handle, main_app_handle];

    if let Some(jh) = maybe_jh {
        handles.push(jh);
    }

    join_all(handles).await;

    Ok(())
}

fn setup_terminal() -> Result<Terminal> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let panic_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic| {
        let _ = disable_raw_mode();
        let _ =
            execute!(io::stderr(), LeaveAlternateScreen, DisableMouseCapture);

        panic_hook(panic);
    }));

    Ok(terminal)
}
