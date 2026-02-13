# Development VM for trying out nix-analytics interactively.
#
# Usage:
#   nix run --impure .#dev-vm
#   # In another terminal:
#   ssh -p 2222 root@localhost    (password: "root")
#
# Inside the VM:
#   nix-analytics                 # launch the TUI
#   nix build --expr '...'        # trigger a build and watch it in the TUI
#   nix-analytics-ctl snapshot    # query the daemon from CLI
#
# Hot-reload workflow (no VM restart needed):
#   Host:  nix develop -c bash -c "cargo build --release --workspace && cd plugin && ninja -C builddir"
#   VM:    reload      # restarts nix-daemon with dev plugin, starts dev daemon
#   VM:    dev-tui     # TUI from dev build
#   VM:    dev-ctl snapshot

{ self, nixpkgs, system, projectDir }:

let
  nixos = nixpkgs.lib.nixosSystem {
    inherit system;
    modules = [
      self.nixosModules.default
      ({ config, pkgs, lib, modulesPath, ... }: {
        imports = [ "${modulesPath}/virtualisation/qemu-vm.nix" ];

        services.nix-analytics = {
          enable = true;
          package = self.packages.${system}.default;
          pluginPackage = self.packages.${system}.nix-analytics-plugin;
        };

        # SSH access.
        services.openssh = {
          enable = true;
          settings = {
            PermitRootLogin = "yes";
            PasswordAuthentication = true;
          };
        };

        users.users.root.password = "root";

        # Nix features needed for nix build inside the VM.
        nix.settings.experimental-features = [ "nix-command" "flakes" "cgroups" "auto-allocate-uids" ];
        nix.settings.use-cgroups = true;
        nix.settings.auto-allocate-uids = true;

        # Allow nix-daemon to create sub-cgroups for freeze/thaw.
        systemd.services.nix-daemon.serviceConfig.Delegate = true;

        # Dev helper scripts — run dev binaries from the shared mount.
        environment.systemPackages = let
          reload = pkgs.writeShellScriptBin "reload" ''
            set -e
            # Stop the Nix-managed daemon
            systemctl stop nix-analyticsd

            # Kill any existing dev daemon
            pkill -f '/mnt/project/target/release/nix-analyticsd' 2>/dev/null || true
            sleep 0.5

            # Point nix-daemon at the dev plugin and restart it
            sed -i 's|^plugin-files = .*|plugin-files = /mnt/project/plugin/builddir/libnix-analytics.so|' /etc/nix/nix.conf
            systemctl restart nix-daemon

            # Start the dev daemon in background
            /mnt/project/target/release/nix-analyticsd &
            echo "reloaded: daemon PID $!, plugin from /mnt/project/plugin/builddir/"
          '';
          dev-tui = pkgs.writeShellScriptBin "dev-tui" ''
            exec /mnt/project/target/release/nix-analytics "$@"
          '';
          dev-ctl = pkgs.writeShellScriptBin "dev-ctl" ''
            exec /mnt/project/target/release/nix-analytics-ctl "$@"
          '';
        in [ reload dev-tui dev-ctl ];

        # QEMU VM settings.
        virtualisation = {
          memorySize = 4096;
          cores = 4;
          # Writable overlay for /nix/store — default is tiny tmpfs.
          diskSize = 102400;  # 100GB disk image
          writableStoreUseTmpfs = false;
          forwardPorts = [
            { from = "host"; host.port = 2222; guest.port = 22; }
          ];
          graphics = false;
          # Mount host project root into the VM for hot-reload.
          sharedDirectories.project = {
            source = projectDir;
            target = "/mnt/project";
            securityModel = "none";
          };
        };

        # Auto-login on serial console as fallback.
        services.getty.autologinUser = "root";

        system.stateVersion = "24.11";
      })
    ];
  };
in
  nixos.config.system.build.vm
