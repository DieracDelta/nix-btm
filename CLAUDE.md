# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

nix-btm is a real-time Nix build monitoring system that provides htop-like visibility into Nix builds. It supports three operating modes: a client-daemon architecture for multiple TUI monitors, or a standalone mode combining both.

**Key Features:**
- Monitor multiple concurrent Nix builds across builders
- Process-level visibility of nixbld workers
- Derivation dependency tree visualization with Eagle Eye View
- Three modes: daemon, client, and standalone
- Lock-free ring buffer IPC with io_uring (Linux) or kqueue (macOS)

## Development Commands

### Building
```bash
just build              # Build workspace in dev mode
cargo build --workspace --profile dev
```

### Testing
```bash
just test               # Run all tests with output
cargo test --workspace --profile dev -- --nocapture

# Run specific integration test
cargo test -p nix-btm --test client_daemon_e2e -- --nocapture
cargo test -p nix-btm --test ringbuffer_e2e -- --nocapture
cargo test -p nix-btm --test rpc_e2e -- --nocapture
cargo test -p nix-btm --test nix_build_integration -- --nocapture
```

### Running

There are three modes of operation:

**Daemon Mode:**
```bash
just run-daemon
# Or:
cargo run --bin nix-btm --profile dev -- daemon -n /tmp/nixbtm.sock -d /tmp/nix-daemon.sock

# With daemonization (double-fork):
cargo run --bin nix-btm -- daemon -f true -n /tmp/nixbtm.sock -d /tmp/nix-daemon.sock
```

**Client Mode:**
```bash
just run-client
# Or:
cargo run --bin nix-btm --profile dev -- client -d /tmp/nix-daemon.sock
```

**Standalone Mode** (simplest, no client-daemon split):
```bash
just run-standalone
# Or:
cargo run --bin nix-btm --profile dev -- standalone -n /tmp/nixbtm.sock
```

### Linting & Formatting
```bash
just fmt                # Format code (cargo fmt + treefmt)
just lint               # Run clippy in release mode
just lint-fix           # Auto-fix clippy warnings
```

### Debugging
```bash
just run-console        # Run tokio-console for async diagnostics
```

### Cleaning
```bash
just clean              # Remove build artifacts
# Manually clean sockets/logs:
rm -f /tmp/nixbtm.sock /tmp/nix-daemon.sock /tmp/nixbtm-*.log
```

## Architecture Overview

### Crate Organization

The workspace contains 3 crates:

1. **crates/nix-btm**: Main binary with all functionality
   - Three views: Builder View, Eagle Eye View, Build Job View
   - Ratatui-based immediate-mode rendering with Gruvbox theme
   - Vim-style keybindings (hjkl navigation, q to quit)
   - Contains both daemon and client code
   - Ring buffer IPC implementation (ring_writer.rs, ring_reader.rs)
   - Protocol definitions (protocol_common.rs)
   - Nix log parser (handle_internal_json.rs)
   - Derivation dependency tree (derivation_tree.rs, tree_generation.rs)
   - RPC protocol for client-daemon communication (rpc.rs, rpc_client.rs, rpc_daemon.rs)

2. **crates/json_parser**: Nix internal-json format parsing
   - Defines Nix log message types
   - ActivityType enum and parsing

3. **crates/clipboard**: OSC 52 terminal clipboard integration
   - Cross-platform clipboard support for TUI

### Operating Modes

The single nix-btm binary can operate in three modes (selected via CLI subcommand):

**Daemon Mode:**
- Listens on Unix socket for Nix internal-json logs (default: /tmp/nixbtm.sock)
- Parses logs and updates JobsStateInner state
- Writes updates to ring buffer for client consumption
- Exposes RPC socket for client connections (default: /tmp/nix-daemon.sock)
- Supports optional daemonization via double-fork

**Client Mode:**
- Connects to daemon via RPC socket
- Requests ring buffer info and initial snapshot
- Reads incremental updates from ring buffer
- Displays TUI (three views: Builder, Eagle Eye, Build Jobs)

**Standalone Mode:**
- Combines daemon and client in a single process
- Simpler architecture, no IPC overhead
- Useful for single-user monitoring

### Client-Daemon Communication

**Data Flow (Daemon + Client mode):**
1. Nix outputs internal-json logs to `/tmp/nixbtm.sock` (configured via `json-log-path` in nix.conf)
2. Daemon parses logs and updates JobsStateInner state via watch channel
3. Daemon detects state changes and writes CBOR-serialized Update messages to ring buffer
4. Daemon uses io_uring futex (Linux) or kqueue (macOS) to wake waiting clients
5. Clients read ring buffer incrementally and apply updates to local state

**RPC Protocol:**
- Client connects to daemon via Unix socket
- ClientRequest::RequestRing → DaemonResponse::RingReady (shm name + size)
- ClientRequest::RequestSnapshot → DaemonResponse::SnapshotReady (snapshot shm name + sequence number)
- Snapshot contains full JobsStateInner serialized in CBOR
- After snapshot, client syncs ring reader to snapshot sequence number and streams updates

### Ring Buffer IPC

The ring buffer is a lock-free, memory-mapped circular buffer:

- **Location**: `crates/nix-btm/src/ring_writer.rs` and `ring_reader.rs`
- **Shared Memory**: Uses POSIX shared memory (shm_open/shm_unlink)
- **Mechanism**:
  - Atomic sequence number (u64) per update
  - Fixed-size circular buffer that wraps around
  - io_uring futex (Linux) or kqueue (macOS) for efficient wake notifications
  - CBOR serialization for Update messages
- **Synchronization**: CAS on AtomicU64 for lock-free writes
- **Update Types**: Defined in protocol_common.rs
  - JobNew(BuildJob): New build job started
  - JobUpdate { jid, status }: Job status changed
  - JobFinish { jid, stop_time_ns }: Job completed
  - DepGraphUpdate { drv, deps }: New dependency node
  - Heartbeat { daemon_seq }: Keep-alive message

### Key Data Structures

**JobsStateInner** (`handle_internal_json.rs`):
- Central state maintained by daemon and replicated in clients
- `jid_to_job: HashMap<JobId, BuildJob>` - all active jobs indexed by JobId
- `drv_to_jobs: HashMap<Drv, HashSet<JobId>>` - jobs grouped by derivation
- `dep_tree: DrvRelations` - dependency graph
- `top_level_targets: Vec<String>` - flake refs being built (e.g., "github:nixos/nixpkgs#bat")
- `drv_to_target: HashMap<Drv, String>` - maps drvs to human-readable names
- Updated by parsing Nix ActivityType messages

**BuildJob** (`handle_internal_json.rs`):
- Tracks individual build activity
- Fields: JobId, RequesterId, Drv, JobStatus, start_time_ns, stop_time_ns

**DrvRelations** (`derivation_tree.rs`):
- Dependency graph of derivations
- `nodes: BTreeMap<Drv, DrvNode>` - all derivation nodes
- `tree_roots: BTreeSet<Drv>` - top-level drvs with no parents
- Used for Eagle Eye View tree visualization
- Tree generation logic in `tree_generation.rs` (pruning, traversal)

**Update enum** (`protocol_common.rs`):
- Message types sent through ring buffer
- CBOR-serialized with bytemuck for efficient encoding
- Includes sequence numbers for ordering and detecting lost updates

### Shutdown Mechanism

The project uses a unified `Shutdown` struct (`shutdown.rs`):
- Clone-able handle that coordinates graceful shutdown across all async tasks
- `trigger()` sets shutdown flag and wakes all waiting tasks
- `wait()` returns a future that resolves when shutdown is triggered
- `is_shutdown()` synchronously checks if shutdown has been triggered
- All async tasks tokio::select! on `shutdown.wait()` to ensure clean termination
- Internally uses Arc<Inner> with Notify + AtomicBool for coordination
- Prevents socket blocking and ensures graceful cleanup

## Platform Specifics

- **Linux**: Full support including ring buffer IPC with io_uring futex
- **macOS**: Ring buffer uses kqueue for notifications
- Process monitoring uses procfs on Linux, sysinfo for cross-platform fallback

## Important Implementation Details

### Nix Log Parsing
- Requires Nix builds run with `-vvv` flag for verbose logging
- Parser in `handle_internal_json.rs` matches on ActivityType (Build, QueryPathInfo, CopyPath, etc.)
- State machine: Starting → BuildPhaseType → Completed
- Regex extraction for dependency discovery from verbose messages
- Extracts flake/attribute references for display (e.g., "github:nixos/nixpkgs#bat")

### TUI Architecture
The client runs multiple concurrent tasks in tokio:
1. **Ring buffer reader** (blocking thread pool): polls ring buffer, applies updates via watch::channel
2. **Process stats poller** (async): polls sysinfo every 1s for nixbld process metrics
3. **Keyboard input handler** (async): handles crossterm events, vim keybindings
4. **TUI renderer** (async): event-driven rendering, not polling

See `event_loop.rs` for the main select! loop coordinating these tasks.

### Performance Optimizations
- **MiMalloc allocator** for faster allocation
- **Release profile**: LTO=fat, single codegen unit, opt-level=3, stripped binaries
- **io_uring** (Linux) / **kqueue** (macOS) for kernel-level async (no polling)
- **Lock-free ring buffer**: atomic CAS only, no mutexes in hot path
- **CBOR** for compact binary serialization (vs JSON)
- **Non-blocking sockets**: recent commit ensures sockets never block
- **Parallel task spawning**: each Nix log line processed in separate task to avoid blocking

### Tree Generation (`tree_generation.rs`)
Recently extracted from derivation_tree.rs:
- Pruning logic: removes uninteresting internal nodes
- PruneType enum: All, OnlyCompleted, None
- Tree traversal for rendering in TUI

## Testing

Tests are in `crates/nix-btm/tests/`:

### End-to-End Tests
- **client_daemon_e2e.rs**: Tests full client-daemon protocol
  - `test_full_client_daemon_flow`: Complete RPC + ring buffer + snapshot flow
  - `test_ring_buffer_job_updates_flow`: Verify job updates propagate correctly
  - `test_multiple_clients_read_ring`: Multiple clients reading same ring buffer
- **ringbuffer_e2e.rs**: Ring buffer correctness tests
  - Wraparound behavior
  - Multiple readers/single writer
  - Sequence number handling
- **rpc_e2e.rs**: RPC protocol tests
- **nix_build_integration.rs**: Integration tests with real Nix builds

### Test Patterns
- Use `make_test_job()` and `make_test_state()` helpers
- Clean up shared memory with `shm_cleanup()` after tests
- Use tempfile for socket paths to avoid conflicts

## Common Development Patterns

### Adding a new Update type
1. Add variant to `Update` enum in `protocol_common.rs`
2. Implement Serialize/Deserialize (ensure CBOR compatibility)
3. In daemon (main.rs `run_daemon` update-writer task): detect state change and encode update
4. In client (main.rs `apply_update`): handle update and modify JobsStateInner
5. In tests: add test case to verify propagation
6. Update TUI rendering in `ui.rs` if needed

### Modifying the Nix log parser
1. Edit `handle_internal_json.rs:handle_line_parsed()`
2. Match on new ActivityType or message patterns
3. Update JobsStateInner via state_tx watch channel
4. Daemon update-writer will automatically detect change and propagate
5. Add test case with sample log line

### Adding a new TUI view
1. Add variant to SelectedTab enum in `app.rs`
2. Implement rendering in `ui.rs:render_tab_content()`
3. Add keyboard navigation in `event_loop.rs` (handle tab switching)
4. Consider state needed in App struct
5. Update help text/keybindings display

### Debugging Ring Buffer Issues
1. Check ring buffer creation: `RingWriter::create()` logs shm name
2. Verify client can open: `RingReader::from_name()`
3. Check sequence numbers: reader should sync to snapshot seq
4. Watch for `ReadResult::Lost` or `ReadResult::NeedCatchup`
5. Use tracing logs (set `RUST_LOG=debug`)

## Build Configuration

- **Edition**: Rust 2024
- **Key dependencies**:
  - Tokio with rt-multi-thread, net, io-util, macros, sync, time, process, tracing, signal
  - io_uring (Linux only) for efficient futex
  - serde_cbor for binary serialization
  - ratatui for TUI rendering
  - crossterm for terminal control
  - tui-tree-widget for tree views
  - sysinfo for process stats
  - procfs (Linux only) for detailed process info
  - rustix for low-level POSIX APIs
  - psx-shm for POSIX shared memory

- **Dev profile**: incremental builds, panic=abort
- **Release profile**: LTO=fat, single codegen unit, opt-level=3, stripped binaries

## Current Development Status

Based on recent commits:
- Architecture refactored: merged daemon, client, and common crates into single nix-btm crate
- Clean shutdown mechanism implemented using Arc<Notify> + AtomicBool
- Concurrency improved: sockets never block, each log line processed in parallel
- Tree generation logic separated into tree_generation.rs
- Ring buffer and RPC protocol working on Linux
- Attribute detection improved (displays human-readable flake refs)
- Build status detection enhanced

## Troubleshooting

### Common Issues

**Socket permission denied:**
```bash
sudo rm /tmp/nixbtm.sock
# Then restart daemon with proper permissions (0o666 in code)
```

**Client can't connect:**
```bash
# Check if daemon is running:
lsof /tmp/nix-daemon.sock
# Check logs:
ls /tmp/nixbtm-daemon-*.log
```

**Ring buffer not found:**
```bash
# Check /dev/shm on Linux:
ls -la /dev/shm/nix-btm-ring-*
# On macOS, check with ipcs command
```

**Nix not sending logs:**
Ensure `json-log-path = /tmp/nixbtm.sock` in `/etc/nix/nix.conf` and run builds with `-vvv`
