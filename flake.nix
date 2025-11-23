{
  description = "Nix Btm";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      fenix,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      forAllSystems =
        f:
        builtins.listToAttrs (
          map (system: {
            name = system;
            value = f system;
          }) systems
        );
    in
    let
      perSystem =
        system:
        let
          # Recreate your arch/os extraction
          info = builtins.split "\([a-zA-Z0-9_]+\)" system;
          arch = (builtins.elemAt (builtins.elemAt info 1) 0);
          os = (builtins.elemAt (builtins.elemAt info 3) 0);

          pkgs = import nixpkgs {
            inherit system;
            overlays = [ (import rust-overlay) ];
            config = {
              allowUnfree = true;
            };
          };
          target = if os == "linux" then "${arch}-unknown-${os}-musl" else "${arch}-apple-darwin";
          maybeWildFlag = if os == "darwin" then "" else " -C link-arg=--ld-path=${pkgs.wild}/bin/wild ";
          maybeWild = if os == "darwin" then [ ] else [ pkgs.wild ];

          rust_tc = with pkgs; [
            (rust-bin.stable.latest.minimal.override {
              extensions = [
                "cargo"
                "clippy"
                "rust-src"
                "rustc"
                "llvm-tools-preview"
              ];
              targets = [ target ];
            })
            (rust-bin.nightly.latest.minimal.override {
              extensions = [ "rustfmt" ];
              targets = [ target ];
            })
          ];

          # maybe_libiconv = pkgs.lib.optional (os == "darwin") (
          #   with pkgs;
          #   [
          #     libiconv
          #     libiconv.dev
          #   ]
          # );

          maybe_hardcoded_hack = if os == "darwin" then " -L/opt/homebrew/lib" else "";

          nix-btm =
            with if os == "darwin" then pkgs else pkgs.pkgsMusl;
            rustPlatform.buildRustPackage {
              pname = "nix-btm";
              version = "0.3.0";

              src = ./.;
              buildAndTestSubdir = "./crates/client";

              doCheck = false;

              cargoLock = {
                lockFile = ./Cargo.lock;
              };

              env = {
                # requires features: sync_unsafe_cell, unbounded_shifts, let_chains, ip
                RUSTC_BOOTSTRAP = 1;
                RUSTFLAGS = "-C target-feature=+crt-static";
                NIX_CFLAGS_COMPILE = "-Wno-error";
                CARGO_BUILD_TARGET = target;
              };

              meta = with lib; {
                description = "Rust tool to monitor nix processes";
                homepage = "https://github.com/DieracDelta/nix-btm";
                license = licenses.gpl3;
                mainProgram = "nix-btm";
              };
            };

          nix-btm-daemon =
            with if os == "darwin" then pkgs else pkgs.pkgsMusl;
            rustPlatform.buildRustPackage {
              pname = "nix-btm-daemon";
              version = "0.3.0";

              src = ./.;
              buildAndTestSubdir = "./crates/daemon";

              doCheck = false;

              cargoLock = {
                lockFile = ./Cargo.lock;
              };

              env = {
                # requires features: sync_unsafe_cell, unbounded_shifts, let_chains, ip
                RUSTC_BOOTSTRAP = 1;
                RUSTFLAGS = "-C target-feature=+crt-static";
                NIX_CFLAGS_COMPILE = "-Wno-error";
                CARGO_BUILD_TARGET = target;
              };

              meta = with lib; {
                description = "Daemon for nix-btm to monitor nix processes";
                homepage = "https://github.com/DieracDelta/nix-btm";
                license = licenses.gpl3;
                mainProgram = "nix-btm-daemon";
              };
            };

          consoleShell = pkgs.mkShell.override { } {
            hardeningDisable = [ "fortify" ];
            RUSTFLAGS = "-C target-feature=+crt-static --cfg tokio_unstable" + maybe_hardcoded_hack;
            CARGO_BUILD_TARGET = target;
            shellHook = ''
              export CARGO_TARGET_DIR="$(git rev-parse --show-toplevel)/target_dirs/nix_rustc";
            '';
            TOKIO_CONSOLE_BIND = "127.0.0.1:6669";
            TOKIO_CONSOLE_RETENTION = "60s";
            TOKIO_CONSOLE_BUFFER_CAPACITY = "2048";
            RUST_LOG = "info,tokio=trace,runtime=trace";
            RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
            buildInputs =
              with pkgs;
              [
                python3

                just
                libiconv
                cargo-generate
                treefmt
                fenix.packages.${system}.rust-analyzer
                tokio-console
              ]
              ++ rust_tc
              ++ lib.optionals pkgs.stdenv.isDarwin [
                pkgs.libiconv
              ];
          };

          devShell = pkgs.mkShell.override { stdenv = pkgs.clangStdenv; } {
            hardeningDisable = [ "fortify" ];
            RUSTFLAGS = "-C target-feature=+crt-static" + maybe_hardcoded_hack + maybeWildFlag;
            CARGO_BUILD_TARGET = target;
            shellHook = ''
              export CARGO_TARGET_DIR="$(git rev-parse --show-toplevel)/target_dirs/nix_rustc";
            '';
            RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
            buildInputs =
              with pkgs;
              [
                python3
                just
                libiconv
                cargo-generate
                treefmt
                fenix.packages.${system}.rust-analyzer
              ]
              ++ maybeWild
              ++ rust_tc;
            # ++ maybe_libiconv;
          };
        in
        {
          packages = {
            default = nix-btm;
            nix-btm = nix-btm;
            nix-btm-daemon = nix-btm-daemon;
          };
          devShells = {
            default = devShell;
            console = consoleShell;
          };
        };

      all = forAllSystems perSystem;
    in
    {
      packages = builtins.mapAttrs (_: v: v.packages) all;

      devShells = builtins.mapAttrs (_: v: v.devShells) all;

      defaultPackage = builtins.mapAttrs (_: v: v.packages.default) all;

      nixosModules.default =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.services.nix-btm-daemon;
        in
        {
          options.services.nix-btm-daemon = {
            enable = lib.mkEnableOption "nix-btm daemon";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.system}.nix-btm-daemon;
              description = "The nix-btm-daemon package to use";
            };

            nixSocketPath = lib.mkOption {
              type = lib.types.str;
              default = "/tmp/nixbtm.sock";
              description = "Path to the Nix JSON log socket (json-log-path setting)";
            };

            daemonSocketPath = lib.mkOption {
              type = lib.types.str;
              default = "/tmp/nix-daemon.sock";
              description = "Path for the daemon RPC socket for client-daemon communication";
            };

            user = lib.mkOption {
              type = lib.types.str;
              default = "nix-btm";
              description = "User to run the daemon as";
            };

            group = lib.mkOption {
              type = lib.types.str;
              default = "nix-btm";
              description = "Group to run the daemon as";
            };
          };

          config = lib.mkIf cfg.enable {
            users.users.${cfg.user} = lib.mkIf (cfg.user == "nix-btm") {
              isSystemUser = true;
              group = cfg.group;
              description = "nix-btm daemon user";
            };

            users.groups.${cfg.group} = lib.mkIf (cfg.group == "nix-btm") { };

            systemd.services.nix-btm-daemon = {
              description = "nix-btm daemon for monitoring Nix builds";
              wantedBy = [ "multi-user.target" ];
              before = [ "nix-daemon.service" ];
              after = [ "network.target" ];

              serviceConfig = {
                Type = "simple";
                ExecStart = "${cfg.package}/bin/nix-btm-daemon -n ${cfg.nixSocketPath} -d ${cfg.daemonSocketPath}";
                ExecStartPre = "${pkgs.coreutils}/bin/rm -f ${cfg.nixSocketPath} ${cfg.daemonSocketPath}";
                Restart = "on-failure";
                RestartSec = "5s";
                User = cfg.user;
                Group = cfg.group;

                # Hardening options
                ProtectSystem = "strict";
                ProtectHome = true;
                PrivateDevices = true;
                ProtectKernelTunables = true;
                ProtectKernelModules = true;
                ProtectControlGroups = true;
                RestrictNamespaces = true;
                RestrictRealtime = true;
                RestrictSUIDSGID = true;
                MemoryDenyWriteExecute = true;
                LockPersonality = true;
                # TODO it queries nix, so we need to add taht to the path
                # TODO the user and group should be dynamic not static

                # Allow access to /tmp for sockets and /nix/store for derivation info
                ReadWritePaths = [ "/tmp" ];
                ReadOnlyPaths = [ "/nix/store" ];
              };
            };
          };
        };

      # nix-darwin module
      darwinModules.default =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.services.nix-btm-daemon;
        in
        {
          options.services.nix-btm-daemon = {
            enable = lib.mkEnableOption "nix-btm daemon for monitoring Nix builds";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.system}.nix-btm-daemon;
              description = "The nix-btm-daemon package to use";
            };

            nixSocketPath = lib.mkOption {
              type = lib.types.str;
              default = "/tmp/nixbtm.sock";
              description = "Path for the Nix JSON log socket";
            };

            daemonSocketPath = lib.mkOption {
              type = lib.types.str;
              default = "/tmp/nix-daemon.sock";
              description = "Path for the daemon RPC socket";
            };
          };

          config = lib.mkIf cfg.enable {
            launchd.daemons.nix-btm-daemon = {
              serviceConfig = {
                Label = "com.github.dieracdelta.nix-btm-daemon";
                ProgramArguments = [
                  "${cfg.package}/bin/nix-btm-daemon"
                  "-n"
                  cfg.nixSocketPath
                  "-d"
                  cfg.daemonSocketPath
                ];
                RunAtLoad = true;
                KeepAlive = true;
                StandardErrorPath = "/tmp/nix-btm-daemon.err.log";
                StandardOutPath = "/tmp/nix-btm-daemon.out.log";
              };
            };
          };
        };
    };
}
