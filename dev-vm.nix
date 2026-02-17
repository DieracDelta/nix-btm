# Development VM for trying out nix-btm interactively.
#
# Usage:
#   nix run --impure .#dev-vm              # standard multi-user (nix-daemon)
#   nix run --impure .#dev-vm-root         # nix as root, no daemon
#   nix run --impure .#dev-vm-daemonless   # daemonless / rootless sockets
#
#   # In another terminal:
#   ssh -p 2222 root@localhost    (password: "root")
#
# Inside the VM (default):
#   nix-btm                 # launch the TUI
#   nix build --expr '...'        # trigger a build and watch it in the TUI
#   nix-btm-ctl snapshot    # query the daemon from CLI
#
# Hot-reload workflow (no VM restart needed):
#   Host:  nix develop -c bash -c "cargo build --release --workspace && cd plugin && ninja -C builddir"
#   VM:    reload      # restarts nix-daemon with dev plugin, starts dev daemon
#   VM:    dev-tui     # TUI from dev build
#   VM:    dev-ctl snapshot

{ self, nixpkgs, system, projectDir, scenario ? "default" }:

let
  nixos = nixpkgs.lib.nixosSystem {
    inherit system;
    modules = [
      self.nixosModules.default
      ({ config, pkgs, lib, modulesPath, ... }: {
        imports = [ "${modulesPath}/virtualisation/qemu-vm.nix" ];

        config = lib.mkMerge [

          # ── Shared base config ──────────────────────────────────────

          {
            services.nix-btm = {
              enable = true;
              package = self.packages.${system}.default;
              pluginPackage = self.packages.${system}.nix-btm-plugin;
            };

            services.openssh = {
              enable = true;
              settings = {
                PermitRootLogin = "yes";
                PasswordAuthentication = true;
              };
            };

            users.users.root.password = "root";

            nix.settings.experimental-features = [ "nix-command" "flakes" "cgroups" "auto-allocate-uids" ];
            nix.settings.use-cgroups = true;
            nix.settings.auto-allocate-uids = true;

            virtualisation = {
              memorySize = 4096;
              cores = 4;
              diskSize = 102400;
              writableStoreUseTmpfs = false;
              forwardPorts = [
                { from = "host"; host.port = 2222; guest.port = 22; }
              ];
              graphics = false;
              sharedDirectories.project = {
                source = projectDir;
                target = "/mnt/project";
                securityModel = "none";
              };
            };

            services.getty.autologinUser = "root";
            system.stateVersion = "24.11";
          }

          # ── default: standard multi-user with nix-daemon ────────────

          (lib.mkIf (scenario == "default") {
            systemd.services.nix-daemon.serviceConfig.Delegate = true;

            environment.systemPackages = let
              reload = pkgs.writeShellScriptBin "reload" ''
                set -e
                systemctl stop nix-btmd
                pkill -f '/mnt/project/target/release/nix-btmd' 2>/dev/null || true
                sleep 0.5
                sed -i 's|^plugin-files = .*|plugin-files = /mnt/project/plugin/builddir/libnix-btm.so|' /etc/nix/nix.conf
                systemctl restart nix-daemon
                /mnt/project/target/release/nix-btmd &
                echo "reloaded: daemon PID $!, plugin from /mnt/project/plugin/builddir/"
              '';
              dev-tui = pkgs.writeShellScriptBin "dev-tui" ''
                exec /mnt/project/target/release/nix-btm "$@"
              '';
              dev-ctl = pkgs.writeShellScriptBin "dev-ctl" ''
                exec /mnt/project/target/release/nix-btm-ctl "$@"
              '';
            in [ reload dev-tui dev-ctl ];
          })

          # ── root: nix as root directly, no nix-daemon ───────────────

          (lib.mkIf (scenario == "root") {
            systemd.sockets.nix-daemon.enable = false;
            systemd.services.nix-daemon.enable = false;
            nix.settings.sandbox = false;

            environment.systemPackages = let
              reload = pkgs.writeShellScriptBin "reload" ''
                set -e
                systemctl stop nix-btmd
                pkill -f '/mnt/project/target/release/nix-btmd' 2>/dev/null || true
                sleep 0.5
                sed -i 's|^plugin-files = .*|plugin-files = /mnt/project/plugin/builddir/libnix-btm.so|' /etc/nix/nix.conf
                systemctl restart nix-btmd
                echo "reloaded: daemon restarted, plugin from /mnt/project/plugin/builddir/"
              '';
              dev-tui = pkgs.writeShellScriptBin "dev-tui" ''
                exec /mnt/project/target/release/nix-btm "$@"
              '';
              dev-ctl = pkgs.writeShellScriptBin "dev-ctl" ''
                exec /mnt/project/target/release/nix-btm-ctl "$@"
              '';
            in [ reload dev-tui dev-ctl ];
          })

          # ── daemonless: user-scoped sockets, manual daemon ──────────

          (lib.mkIf (scenario == "daemonless") {
            systemd.sockets.nix-daemon.enable = false;
            systemd.services.nix-daemon.enable = false;
            nix.settings.sandbox = false;

            # Disable the system nix-btmd — we run it manually.
            systemd.services.nix-btmd.enable = false;

            # Point the plugin at the user-scoped socket.
            nix.extraOptions = lib.mkForce ''
              btm-socket = /run/user/0/nix-btm/events.sock
            '';

            # Point TUI/ctl at the user-scoped socket dir.
            environment.sessionVariables.NIX_BTM_SOCKET_DIR = lib.mkForce "/run/user/0/nix-btm";

            environment.systemPackages = let
              start-daemon = pkgs.writeShellScriptBin "start-daemon" ''
                set -e
                SOCK_DIR="/run/user/0/nix-btm"
                mkdir -p "$SOCK_DIR"

                pkill -f 'nix-btmd' 2>/dev/null || true
                sleep 0.3

                export NIX_BTM_SOCKET_DIR="$SOCK_DIR"
                ${self.packages.${system}.default}/bin/nix-btmd &
                echo "nix-btmd started (PID $!), sockets in $SOCK_DIR"
              '';
              daemonless-build = pkgs.writeShellScriptBin "daemonless-build" ''
                exec nix build --option sandbox false "$@"
              '';
              start-dev-daemon = pkgs.writeShellScriptBin "start-dev-daemon" ''
                set -e
                SOCK_DIR="/run/user/0/nix-btm"
                mkdir -p "$SOCK_DIR"

                pkill -f 'nix-btmd' 2>/dev/null || true
                sleep 0.3

                sed -i 's|^plugin-files = .*|plugin-files = /mnt/project/plugin/builddir/libnix-btm.so|' /etc/nix/nix.conf

                export NIX_BTM_SOCKET_DIR="$SOCK_DIR"
                /mnt/project/target/release/nix-btmd &
                echo "dev daemon started (PID $!), sockets in $SOCK_DIR"
              '';
              dev-tui = pkgs.writeShellScriptBin "dev-tui" ''
                export NIX_BTM_SOCKET_DIR="/run/user/0/nix-btm"
                exec /mnt/project/target/release/nix-btm "$@"
              '';
              dev-ctl = pkgs.writeShellScriptBin "dev-ctl" ''
                export NIX_BTM_SOCKET_DIR="/run/user/0/nix-btm"
                exec /mnt/project/target/release/nix-btm-ctl "$@"
              '';
            in [ start-daemon daemonless-build start-dev-daemon dev-tui dev-ctl ];
          })

        ];  # end mkMerge
      })
    ];
  };
in
  nixos.config.system.build.vm
