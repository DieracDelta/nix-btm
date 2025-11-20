# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

nix-btm is a real-time Nix build monitoring system that provides htop-like visibility into Nix builds. It uses a client-daemon architecture with a lock-free ring buffer IPC mechanism to enable multiple TUI clients to monitor concurrent Nix builds efficiently.

**Key Features:**
- Monitor multiple concurrent Nix builds across builders
- Process-level visibility of nixbld workers
- Derivation dependency tree visualization
- Client-daemon architecture with shared memory IPC

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

# Run tests for a specific crate
cargo test -p nix-btm-common --profile dev -- --nocapture

# Run a specific test
cargo test -p nix-btm-common test_name --profile dev -- --nocapture
```

### Running
```bash
# Start the daemon (currently runs client mode, see justfile)
just run-daemon

# Start the client
just run-client

# Or run directly with cargo
cargo run --bin nix-btm --profile dev -- client -n /tmp/nixbtm.sock
```

### Linting & Formatting
```bash
just fmt                # Format code (cargo fmt + treefmt)
just lint               # Run clippy in release mode
just lint-fix           # Auto-fix clippy warnings
```

### Cleaning
```bash
just clean              # Remove build artifacts
```

## Architecture Overview

### Crate Organization

The workspace contains 5 crates:

1. **crates/client** (nix-btm): The TUI client binary
   - Three views: Builder View, Eagle Eye View, Build Job View
   - Ratatui-based immediate-mode rendering
   - Vim-style keybindings (hjkl navigation)
   - Connects to daemon via ring buffer + shared memory

2. **crates/daemon** (nix-btm-daemon): Daemon binary (in development)
   - Listens on Unix socket for Nix internal-json logs
   - Manages JobsStateInner state
   - Writes updates to ring buffer

3. **crates/nix-btm-common**: Shared protocol and IPC code
   - Ring buffer implementation (RingWriter/RingReader)
   - Protocol definitions (protocol_common.rs)
   - Nix log parser (handle_internal_json.rs)
   - Snapshot/catchup mechanism
   - Derivation dependency tree (derivation_tree.rs)

4. **crates/json_parser**: Nix internal-json format parsing
   - Defines Nix log message types
   - ActivityType enum and parsing

5. **crates/clipboard**: OSC 52 terminal clipboard integration

### Client-Daemon Communication

**Data Flow:**
1. Nix outputs internal-json logs to `/tmp/nixbtm.sock` (configured via `json-log-path` in nix.conf)
2. Daemon parses logs and updates JobsStateInner state
3. Daemon writes CBOR-serialized updates to ring buffer every 1s
4. Daemon uses io_uring futex to wake waiting clients
5. Clients read ring buffer incrementally and update TUI

**Catchup Protocol:**
- New clients request snapshot via shared memory (nix-btm-snapshot-p{pid})
- Snapshot contains full JobsStateInner state in CBOR format
- After catchup, clients stream incremental updates from ring buffer

### Ring Buffer IPC (Linux-only)

The ring buffer is a lock-free, memory-mapped circular buffer:

- **Location**: `crates/nix-btm-common/src/ring_writer.rs` and `ring_reader.rs`
- **Mechanism**:
  - Atomic sequence number + offset in header
  - Fixed-size circular buffer (wraps around)
  - io_uring futex for efficient wake notifications
  - CBOR serialization for updates
- **Synchronization**: CAS on AtomicU64 for lock-free writes
- **Update Types**: Defined in protocol_common.rs (JobNew, JobUpdate, JobFinish, etc.)

### Key Data Structures

**JobsStateInner** (`handle_internal_json.rs`):
- Central state maintained by daemon
- HashMap<JobId, BuildJob> for all active jobs
- BuildJob tracks: JobId, Drv, status, timing, requester
- Updated by parsing Nix ActivityType messages

**DrvRelations** (`derivation_tree.rs`):
- Dependency graph of derivations
- BTreeMap<Drv, DrvNode> with parent/child relationships
- Used for Eagle Eye View tree visualization

**Update enum** (`protocol_common.rs`):
- Message types sent through ring buffer
- Variants: JobNew, JobUpdate, JobFinish, JobRemove, etc.
- CBOR-serialized with bytemuck for zero-copy

## Platform Specifics

- **Linux**: Full support including ring buffer IPC with io_uring futex
- **macOS**: Partial support (ring buffer IPC not yet implemented, use basic socket mode)
- Process monitoring uses procfs on Linux, sysinfo for cross-platform fallback

## Important Implementation Details

### Nix Log Parsing
- Requires Nix builds run with `-vvv` flag for verbose logging
- Parser matches on ActivityType (Build, QueryPathInfo, CopyPath, etc.)
- State machine: Starting → BuildPhase → Completed
- Regex extraction for dependency discovery from verbose messages

### TUI Event Loop
The client runs multiple concurrent tasks in tokio:
1. Ring buffer reader (watch::channel updates)
2. Keyboard input handler (crossterm events)
3. Process stats poller (every 1s via sysinfo)
4. TUI renderer (event-driven, not polling)

See `crates/client/src/event_loop.rs` for the main select! loop.

### Performance Optimizations
- MiMalloc allocator for faster malloc
- Release profile: LTO=fat, single codegen unit, opt-level=3
- io_uring for kernel-level async (no polling)
- Lock-free ring buffer (atomic CAS only)
- CBOR for compact serialization vs JSON

## Testing the Ring Buffer

Ring buffer tests are in `crates/nix-btm-common/tests/`. To run:

```bash
cargo test -p nix-btm-common --profile dev -- --nocapture
```

Key test patterns:
- Create RingWriter with memfd
- Spawn RingReader tasks
- Write updates with record()
- Verify readers receive in order
- Test wraparound behavior

## Common Development Patterns

### Adding a new Update type
1. Add variant to `Update` enum in `protocol_common.rs`
2. Implement serde Serialize/Deserialize
3. Handle in daemon: encode and write to ring buffer
4. Handle in client: match on Kind and update app state
5. Update TUI rendering in `ui.rs` if needed

### Modifying the Nix log parser
1. Edit `handle_internal_json.rs`
2. Match on new ActivityType or message patterns
3. Update JobsStateInner state accordingly
4. Consider if update should propagate to ring buffer
5. Add tests for new parsing logic

### Adding a new TUI view
1. Add variant to SelectedTab enum in `main.rs`
2. Implement rendering in `ui.rs` render_tab_content()
3. Add keyboard navigation in `event_loop.rs`
4. Consider state needed in App struct

## Build Configuration

The project uses Rust edition 2024 and requires specific features:
- Tokio with rt-multi-thread, net, io-util, macros, sync, time, process, tracing
- io_uring for Linux futex support
- serde_cbor for binary serialization
- ratatui for TUI rendering

Dev profile: incremental builds, panic=abort
Release profile: LTO, single codegen unit, stripped binaries

## Current Development Status

Based on recent commits:
- Ring buffer implementation is compiled and ready for testing
- Client-daemon architecture transitioning to u64 atomic sequences
- macOS builds re-enabled but ring buffer is Linux-only
- Fast linker (wild) enabled for quicker iteration
