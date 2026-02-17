# The Catchy Marketing

Did nix just mass-spawn 200 build processes across 47 derivations, max out your cores, eat 32GB of RAM, and then disappear before you could figure out which `nix build` caused it? Did the OOM killer take out your browser, your IDE, and your will to live?

Have you ever `kill -9`'d a nix-daemon fork and prayed? Have you ever wanted to freeze a build, check what it's doing, then let it continue? Have you ever wished you could see which *user* launched that 3-hour kernel rebuild on your shared builder?

Do you run a fleet of remote builders and have no idea what's actually happening on them? Does Hydra tell you a build failed 40 minutes ago but not what went wrong in real time?

nix-btm is `htop` for nix. `nom`, but global, interactive, and with cgroup-level control. A daemon that hooks directly into the nix process via a logger plugin, captures every build event as it happens, and serves it to a terminal UI where you can monitor, freeze, kill, nice, memory-limit, and inspect builds.

# Architecture

Three components:

1. **C++ plugin** (`libnix-btm.so`) -- loaded into the nix process via `plugin-files`, wraps the nix Logger, sends structured JSON events over a Unix socket
2. **Rust daemon** (`nix-btmd`) -- receives events, discovers cgroups, polls remote builders, serves a control socket
3. **TUI** (`nix-btm`) -- connects to the daemon, renders build state, lets you interact with builds

The plugin captures activity start/stop, phase changes, log lines, progress, post-build-hook output, and remote dispatch events. The daemon correlates these with cgroup data to give you per-build CPU, memory, freeze state, and process trees. The TUI gives you vim-style keybindings to navigate, filter, kill, freeze, and yank build info.

# How to use

## NixOS (multi-user nix-daemon, the common case)

Add the flake and enable the module:

```nix
# flake.nix
{
  inputs.nix-btm.url = "github:DieracDelta/nix-btm-mark-2";

  outputs = { self, nixpkgs, nix-btm, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        nix-btm.nixosModules.default
        {
          services.nix-btm = {
            enable = true;
            package = nix-btm.packages.x86_64-linux.default;
            pluginPackage = nix-btm.packages.x86_64-linux.nix-btm-plugin;
          };
        }
      ];
    };
  };
}
```

Rebuild, then:

```bash
# launch the TUI
nix-btm

# or query from scripts
nix-btm-ctl list-builds
nix-btm-ctl get-history
nix-btm-ctl snapshot
```

This sets up everything: the plugin loads into nix-daemon, cgroups are enabled, the daemon runs as a systemd service, sockets live in `/run/nix-btm/`.

## Running nix as root (no daemon)

If you run `nix build` as root directly (not through nix-daemon), the plugin still loads and sends events. The daemon still discovers cgroups -- it finds them via `/proc/<pid>/cgroup` for each nix process, which works regardless of whether builds appear under a systemd service cgroup or a user session scope.

Same NixOS module config as above. The only difference is that builds show up under your session's cgroup hierarchy instead of `nix-daemon.service`.

## Rootless nix (per-user install, no root)

Rootless nix can't write to `/run/nix-btm/` and the daemon can't run as a system service. You need to point everything at a user-writable directory.

The daemon, TUI, and ctl tool all respect the `NIX_BTM_SOCKET_DIR` environment variable. The resolution order is:

1. `NIX_BTM_SOCKET_DIR` (explicit override)
2. `$XDG_RUNTIME_DIR/nix-btm` (user-scoped default)
3. `/run/nix-btm` (system default)

For rootless nix, set the env var and tell the plugin where to send events:

```bash
# In your shell profile
export NIX_BTM_SOCKET_DIR="$XDG_RUNTIME_DIR/nix-btm"
```

```ini
# In ~/.config/nix/nix.conf
plugin-files = /path/to/libnix-btm.so
use-cgroups = true
btm-socket = /run/user/1000/nix-btm/events.sock
```

Then run the daemon manually (or as a systemd user service):

```bash
nix-btmd
```

**Cgroup control caveat**: kill/freeze/nice/memory-limit require write access to cgroup files. Under rootless nix, this depends on whether systemd delegates cgroup control to your user. If it doesn't, you can still monitor builds -- you just can't control them via cgroups. The daemon falls back to PID-based signals (`SIGKILL`/`SIGSTOP`/`SIGCONT`) when cgroups aren't available.

## NixOS with custom socket directory

The module exposes a `socketDir` option:

```nix
services.nix-btm = {
  enable = true;
  socketDir = "/run/custom-btm";
};
```

This sets `NIX_BTM_SOCKET_DIR` on the systemd service and system-wide session variables, creates the directory via tmpfiles, and configures the plugin's `btm-socket` in nix.conf.

# TUI Keybindings

| Key | Action |
|-----|--------|
| `j`/`k` | Navigate up/down |
| `Ctrl-u`/`Ctrl-d` | Half page up/down |
| `gg`/`G` | Jump to top/bottom |
| `K` | Action prompt (then `k`=kill, `f`=freeze, `u`=unfreeze) |
| `V` | Visual mode (select range) |
| `y` | Yank prompt (copy build field to clipboard) |
| `p` | Process view (per-build process tree) |
| `d` | Dependency tree view |
| `l` | Toggle log panel |
| `h` | Toggle history |
| `r` | Toggle remote machines |
| `f` | Filter builds |
| `/` | Search (in dep tree) |
| `n`/`N` | Nice build / search next |
| `c` | CPU limit |
| `m` | Memory limit |
| `a` | Toggle show all activity types |
| `z` then `c`/`o`/`a` | Fold close/open/toggle |
| `q` | Quit |

# Testing

## Unit tests

```bash
nix develop --command bash -c "cargo test --workspace"
```

This runs all unit and integration tests (~142 tests covering protocol encoding, state management, control server request handling, cgroup parsing, machine parsing, UI rendering, and full pipeline integration).

## VM end-to-end test

```bash
nix build .#checks.x86_64-linux.vm-e2e --print-build-logs
```

This spins up two NixOS VMs:

1. **Standard multi-user** -- default `/run/nix-btm/` sockets, verifies empty state, triggers a build, checks history, validates `user_uid` is populated from cgroup discovery
2. **Custom socket directory** -- `socketDir = "/run/custom-btm"`, verifies the daemon binds to the right place, ctl can query it, and builds are tracked

## Nix build + check

```bash
# Build everything
nix build

# Run all checks (unit tests + VM test)
nix flake check
```

## Dev VMs

Three VM scenarios for interactive testing:

```bash
# Standard multi-user (nix-daemon)
nix run --impure .#dev-vm

# Nix as root, no daemon
nix run --impure .#dev-vm-root

# Daemonless / rootless (user-scoped sockets)
nix run --impure .#dev-vm-daemonless
```

SSH in with `ssh -p 2222 root@localhost` (password: `root`).

**dev-vm** (default): Standard NixOS with nix-daemon. Run `nix build` normally, view builds in `nix-btm`.

**dev-vm-root**: nix-daemon is disabled. Run `nix build` as root directly. The plugin still loads and sends events to the system nix-btmd service.

**dev-vm-daemonless**: Both nix-daemon and the system nix-btmd are disabled. Run `start-daemon` to launch nix-btmd with user-scoped sockets, then `daemonless-build --expr '...'` to trigger builds. Demonstrates the full rootless flow.

All three support hot-reload: build on the host, then run `reload` (default/root) or `start-dev-daemon` (daemonless) in the VM.

## Manual testing with custom socket dir

```bash
# Terminal 1
export NIX_BTM_SOCKET_DIR=/tmp/test-btm
cargo run --bin nix-btmd

# Terminal 2
export NIX_BTM_SOCKET_DIR=/tmp/test-btm
cargo run --bin nix-btm-ctl -- list-builds

# Terminal 3
export NIX_BTM_SOCKET_DIR=/tmp/test-btm
cargo run --bin nix-btm
```

# Development

```bash
nix develop
cargo watch -x check
```

The devshell includes the Rust toolchain, rust-analyzer, meson/ninja for the C++ plugin, and cargo-watch.

There is no system cargo -- all cargo commands must go through `nix develop`.
