# nix-analytics: Design Document

## Overview

nix-analytics is a build monitoring and control system for Nix. It consists of three components:

1. **libnix-analytics.so** — A C++ plugin loaded by the nix-daemon that taps into the Logger to emit structured build events
2. **nix-analyticsd** — A Rust daemon that consumes events, maintains live build state, and exposes a query/control API
3. **nix-analytics** — A Rust TUI client for monitoring and controlling builds

No fork of Nix is required. The plugin uses Nix's existing `plugin-files` mechanism.

## Architecture

```
                           ┌─────────────────────────────────────────────────┐
                           │                  USER MACHINE                   │
                           │                                                 │
  ┌──────────┐  unix sock  │  ┌───────────────────────────────────────────┐  │
  │  nix-    │◄────────────┼──┤           nix-analyticsd (Rust)           │  │
  │ analytics│  query API  │  │                                           │  │
  │   (TUI)  │────────────►│  │  ┌─────────────┐  ┌───────────────────┐  │  │
  └──────────┘  commands   │  │  │ BuildState   │  │ Control Actions   │  │  │
     kill,                 │  │  │              │  │                   │  │  │
     renice,               │  │  │ active_builds│  │ kill → cgroup.   │  │  │
     limit                 │  │  │ history      │  │        kill      │  │  │
                           │  │  │ machines     │  │ nice → cpu.weight│  │  │
                           │  │  │ per-build:   │  │ mem  → memory.max│  │  │
                           │  │  │  logs,phase, │  │ core → cpu.max   │  │  │
                           │  │  │  user,cgroup │  │                   │  │  │
                           │  │  └──────▲───────┘  └───────┬───────────┘  │  │
                           │  │         │                   │              │  │
                           │  └─────────┼───────────────────┼──────────────┘  │
                           │            │                   │                  │
                           │   events   │                   │ /sys/fs/cgroup/  │
                           │  (socket)  │                   │  nix-daemon/     │
                           │            │                   │  build-N/...     │
                           │  ┌─────────┴───────────────────▼──────────────┐  │
                           │  │              nix-daemon                     │  │
                           │  │                                            │  │
                           │  │  ┌──────────────────────────────────────┐  │  │
                           │  │  │  libnix-analytics.so  (C++ plugin)  │  │  │
                           │  │  │                                      │  │  │
                           │  │  │  wraps Logger → emits events         │  │  │
                           │  │  │  RegisterSetting("analytics-socket") │  │  │
                           │  │  └──────────────────────────────────────┘  │  │
                           │  │                                            │  │
                           │  │  forks per connection:                      │  │
                           │  │  ┌──────────┐ ┌──────────┐ ┌──────────┐   │  │
                           │  │  │ client A │ │ client B │ │ client C │   │  │
                           │  │  │ uid=1000 │ │ uid=1001 │ │ uid=1000 │   │  │
                           │  │  └────┬─────┘ └────┬─────┘ └────┬─────┘   │  │
                           │  │       │            │            │          │  │
                           │  └───────┼────────────┼────────────┼──────────┘  │
                           │          │            │            │              │
                           └──────────┼────────────┼────────────┼──────────────┘
                                      │            │            │
                              ┌───────▼──┐  ┌──────▼───┐  ┌────▼─────┐
                              │nix build │  │nix build │  │nix build │
                              │(user A)  │  │(user B)  │  │(user A)  │
                              └──────────┘  └──────────┘  └──────────┘
```

## Why the daemon (not the client)

The nix-daemon uses a fork-per-connection model (`src/nix/unix/daemon.cc:297-327`). Each forked child handles one client. Plugins loaded via `plugin-files` are loaded once in the parent daemon process and inherited by every fork.

The daemon is the correct place because it:

- Sees **all** clients' builds (not just one session)
- Knows the **user** identity — PID, UID, username from the unix socket peer info (`daemon.cc:286-295`)
- Owns the **builder processes** — it does the fork/exec for local builds
- Creates **cgroups** per build when `use-cgroups = true` (`daemon.cc:250-267`)
- Spawns **build-remote** for remote builder dispatch — logs flow back through the daemon's Logger
- Has **root privileges** needed for cgroup manipulation

A client-side plugin would only see its own session's builds and would lack access to cgroups, other users' builds, and remote dispatch information.

## Component 1: libnix-analytics.so (C++ Plugin)

### Plugin entry point

The plugin exports `nix_plugin_entry()`, which the daemon calls during `initPlugins()` (`src/libmain/plugin.cc:102-104`). The function:

1. Registers a custom setting `analytics-socket` via Nix's config system
2. Wraps the global `nix::logger` with an `AnalyticsLogger` that intercepts events

### Logger wrapper

Nix's `Logger` class (`src/libutil/include/nix/util/logging.hh:72-175`) has these virtual methods the plugin intercepts:

| Logger method | Event emitted | Data captured |
|---|---|---|
| `startActivity(act, lvl, type, s, fields, parent)` | `build_started` | activity_id, type (actBuild/actSubstitute/etc), drv_path, parent_id |
| `stopActivity(act)` | `build_stopped` | activity_id, timestamp |
| `result(act, resSetPhase, fields)` | `phase_changed` | activity_id, phase name |
| `result(act, resBuildLogLine, fields)` | `log_line` | activity_id, text |
| `result(act, resProgress, fields)` | `progress` | activity_id, done/expected/running/failed |
| `result(act, resPostBuildLogLine, fields)` | `post_build_log` | activity_id, text |

The wrapper delegates all calls to the original logger (so normal Nix behavior is unchanged) and additionally serializes events to the analytics socket.

### Activity types captured

From `src/libutil/include/nix/util/logging.hh:15-30`:

```
actCopyPath     = 100   // copying a store path
actFileTransfer = 101   // downloading
actRealise      = 102   // realising a derivation
actCopyPaths    = 103   // copying multiple paths
actBuilds       = 104   // aggregate builds activity
actBuild        = 105   // individual build
actOptimiseStore= 106
actVerifyPaths  = 107
actSubstitute   = 108   // substituting from cache
actQueryPathInfo= 109
actPostBuildHook= 110
actBuildWaiting = 111   // build waiting for slot
actFetchTree    = 112   // fetching a tree input
```

### Connection info capture

When the daemon forks for a client, the plugin can capture the peer info that the daemon already extracts (`daemon.cc:286-295`):

- Client PID
- Client UID → username
- Whether the client is trusted

This requires the plugin to hook into the connection setup. The simplest approach: the plugin reads `/proc/self/status` in the forked child to identify itself, then correlates with the parent daemon's accepted connection log.

A cleaner approach: extend the event protocol so the daemon's per-connection child emits a `client_connected` event with peer info at the start of each session.

### Wire format

Events are serialized as length-prefixed MessagePack (or protobuf) and written to the Unix socket configured by `analytics-socket`. MessagePack is preferred for simplicity — no schema compilation needed.

```
[4 bytes: length][payload]
```

Each payload:

```json
{
  "type": "build_started",
  "timestamp_us": 1707750000000000,
  "activity_id": 42,
  "activity_type": 105,
  "drv_path": "/nix/store/abc...-foo.drv",
  "description": "building foo-1.0",
  "parent_id": 3
}
```

### Size estimate

~200-300 lines of C++. The plugin is intentionally thin — it does not store state, make decisions, or modify Nix behavior.

## Component 2: nix-analyticsd (Rust)

### Responsibilities

1. Listen on the analytics Unix socket for events from the plugin
2. Maintain a live model of all builds across all clients
3. Poll cgroup filesystem for resource usage stats
4. Parse `/etc/nix/machines` for remote builder info
5. Read `/nix/var/nix/current-load/` slot lock files for remote builder utilization
6. Expose a query/control API to the TUI over a separate Unix socket
7. Execute control actions (kill, renice, limit) via cgroup filesystem

### Data model

```rust
struct AnalyticsState {
    /// Currently active builds, keyed by activity ID
    active_builds: HashMap<u64, Build>,

    /// Recently completed builds (ring buffer)
    history: VecDeque<CompletedBuild>,

    /// Remote builder configuration and live status
    machines: Vec<RemoteMachine>,
}

struct Build {
    activity_id: u64,
    activity_type: u16,        // actBuild, actSubstitute, etc.
    drv_path: String,
    description: String,
    parent_id: Option<u64>,

    // Timing
    started_at: Instant,

    // Build metadata
    phase: Option<String>,     // current phase (unpack, build, install, etc.)
    recent_log: RingBuffer<String>,  // last N log lines

    // Client info
    user: Option<String>,
    user_pid: Option<u32>,

    // Resource tracking (from cgroup)
    cgroup_path: Option<PathBuf>,
    cpu_user: Duration,
    cpu_system: Duration,
    memory_current: Option<u64>,  // bytes

    // Progress
    progress: Option<Progress>,

    // Where it's running
    machine: BuildMachine,     // Local or Remote(uri)
}

struct Progress {
    done: u64,
    expected: u64,
    running: u64,
    failed: u64,
}

enum BuildMachine {
    Local,
    Remote { uri: String, machine_name: String },
}

struct CompletedBuild {
    build: Build,
    finished_at: Instant,
    result: BuildOutcome,      // success/failure + status
    cpu_user_total: Duration,
    cpu_system_total: Duration,
}

struct RemoteMachine {
    store_uri: String,
    system_types: Vec<String>,
    max_jobs: u32,
    speed_factor: f32,
    supported_features: Vec<String>,

    // Live status (from slot lock files)
    active_slots: u32,
    // active_builds on this machine (correlated from events)
    builds: Vec<u64>,          // activity IDs
}
```

### Cgroup integration

When `use-cgroups = true`, the nix-daemon creates cgroups at (`src/nix/unix/daemon.cc:250-267`):

```
/sys/fs/cgroup/<root>/nix-daemon/          # daemon itself
/sys/fs/cgroup/<root>/nix-daemon/build-N/  # per-build
```

The analytics daemon polls these paths for:

| File | Data |
|---|---|
| `cpu.stat` | `usage_usec`, `user_usec`, `system_usec` |
| `memory.current` | current memory usage in bytes |
| `memory.peak` | peak memory usage |
| `cgroup.procs` | PIDs in this cgroup (thread count proxy) |
| `cpu.max` | current CPU limit (readable, writable for control) |
| `memory.max` | current memory limit (readable, writable for control) |
| `cpu.weight` | scheduling weight / niceness (writable for control) |

Nix already reads `cpu.stat` for `CgroupStats` (`src/libutil/linux/cgroup.cc:52-77`) and uses `cgroup.kill` to terminate builds (`cgroup.cc:89-93`).

### Remote builder monitoring

The analytics daemon reads the same data sources that `build-remote` uses:

1. **Machine list**: Parse `/etc/nix/machines` (or `builders` setting) using the same format as `Machine::parseConfig` (`src/libstore/include/nix/store/machines.hh:79`)
2. **Slot utilization**: Check lock files at `/nix/var/nix/current-load/<uri>-<slot>` — a locked file = active build on that slot (`build-remote.cc:40-43, 152-160`)
3. **Build-to-machine correlation**: When the plugin sees a `build_started` event from a `build-remote` child, correlate it with the machine URI emitted in stderr (`build-remote.cc:255`)

### Control API

The analytics daemon listens on a second Unix socket (e.g. `/run/nix-analytics-ctl.sock`) for commands from the TUI:

```
Query commands:
  list_builds          → all active builds with metadata
  get_build(id)        → full detail for one build
  get_build_log(id, n) → last N log lines
  list_machines         → remote builders + load
  get_history(n)       → last N completed builds

Control commands:
  kill_build(id)       → write "1" to cgroup/cgroup.kill
  set_nice(id, weight) → write to cgroup/cpu.weight (1-10000, default 100)
  set_cpu_limit(id, max) → write to cgroup/cpu.max (e.g. "50000 100000" = 50%)
  set_memory_limit(id, bytes) → write to cgroup/memory.max
```

### Permissions

The analytics daemon needs:
- Read access to the event socket (plugin writes to it)
- Read access to `/sys/fs/cgroup/...` for monitoring
- Write access to `/sys/fs/cgroup/.../nix-daemon/build-*/` for control actions
- Read access to `/nix/var/nix/current-load/` for remote builder slots

In practice, it should run as root or in the same cgroup hierarchy as the nix-daemon. A systemd service is the natural deployment.

## Component 3: nix-analytics TUI (Rust)

### UI layout

```
┌─ nix-analytics ────────────────────────────────────────────────┐
│ Builds (3 active, 2 queued)                    [q]uit [k]ill  │
│────────────────────────────────────────────────────────────────│
│ ID  DRV              PHASE     USER   MACHINE   TIME   CPU%   │
│ ►42 firefox-128.0    build     alice  local     4m32s  340%   │
│  43 mesa-24.1        install   bob    build2    1m15s  780%   │
│  44 linux-6.12       build     alice  build3    12m03s 1500%  │
│  39 python-3.12      waiting   carol  -         0m02s  -      │
│  40 gtk4-4.14        waiting   alice  -         0m01s  -      │
│────────────────────────────────────────────────────────────────│
│ Remote Builders                                                │
│  build1  ssh-ng://b1.internal   2/4 slots   x86_64-linux      │
│  build2  ssh-ng://b2.internal   3/8 slots   x86_64-linux      │
│  build3  ssh-ng://b3.internal   1/16 slots  x86_64-linux      │
│────────────────────────────────────────────────────────────────│
│ Log: firefox-128.0 (build phase)                               │
│ > checking for gcc... /nix/store/...-gcc-13/bin/gcc            │
│ > checking for g++... /nix/store/...-gcc-13/bin/g++            │
│ > checking build system type... x86_64-pc-linux-gnu            │
│ > configure: creating ./config.status                          │
│ > config.status: creating Makefile                             │
└────────────────────────────────────────────────────────────────┘
```

### Controls

| Key | Action |
|---|---|
| `↑/↓` | Select build |
| `k` | Kill selected build |
| `n` | Set niceness (prompt for value) |
| `c` | Set CPU limit (prompt for percentage) |
| `m` | Set memory limit (prompt for value) |
| `l` | Toggle log panel for selected build |
| `h` | Show build history |
| `r` | Show remote builders panel |
| `q` | Quit |

### Library choices

- **ratatui** for terminal UI
- **tokio** for async socket communication
- **serde** + **rmp-serde** (MessagePack) for wire format

## User setup

### nix.conf changes (3 lines)

```ini
plugin-files = /path/to/libnix-analytics.so
extra-experimental-features = cgroups
use-cgroups = true
```

### systemd service

```ini
[Unit]
Description=Nix Analytics Daemon
After=nix-daemon.service

[Service]
ExecStart=/path/to/nix-analyticsd
Restart=always

[Install]
WantedBy=multi-user.target
```

### NixOS module (future)

```nix
services.nix-analytics.enable = true;
# Automatically adds plugin-files, enables cgroups,
# sets up systemd service, etc.
```

## Codebase references

Key files in the Nix source that this design depends on:

| File | What it provides |
|---|---|
| `src/libmain/plugin.cc` | Plugin loading via `dlopen` + `nix_plugin_entry()` |
| `src/libutil/include/nix/util/logging.hh` | Logger class, ActivityType enum, ResultType enum |
| `src/libstore/daemon.cc` | Daemon request handling, `SetOptions`, `BuildDerivation` |
| `src/nix/unix/daemon.cc` | Daemon accept loop, fork-per-connection, cgroup setup, peer auth |
| `src/libutil/linux/cgroup.cc` | `getCgroupStats()`, `destroyCgroup()`, cgroup.kill |
| `src/libutil/linux/include/nix/util/cgroup.hh` | `CgroupStats` struct (cpuUser, cpuSystem) |
| `src/libstore/include/nix/store/build-result.hh` | `BuildResult` with timing and CPU stats |
| `src/nix/build-remote/build-remote.cc` | Remote builder dispatch, machine selection, slot locks |
| `src/libstore/include/nix/store/machines.hh` | `Machine` struct (storeUri, maxJobs, speedFactor, etc.) |
| `src/libstore/include/nix/store/globals.hh` | `buildCores`, `maxBuildJobs`, `build-hook`, `post-build-hook` settings |
| `src/libcmd/include/nix/cmd/command.hh` | `RegisterCommand` for adding `nix analytics` subcommand |

## Open questions

1. **Event delivery guarantee**: If the analytics daemon is down, should the plugin buffer events, drop them, or block? Recommendation: non-blocking write with small kernel buffer; drop events if socket is full. The plugin must never slow down builds.

2. **Remote builder deep monitoring**: Should we SSH into remote builders to get their cgroup stats, or just track what we can see from the coordinator side (slot usage, build duration, result)? Phase 1: coordinator-side only. Phase 2: optional agent on remote builders.

3. **Historical data persistence**: Should nix-analyticsd persist build history to disk (SQLite)? Recommendation: yes, for post-hoc analysis. The TUI can show "last N builds" and query historical stats.

4. **Multi-user auth for TUI**: Should the TUI enforce that user A can only kill user A's builds? Recommendation: yes, check the TUI client's UID against the build's user. Root can control everything.
