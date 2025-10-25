{
  description = "Nix Btm";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs@{
      self,
      nixpkgs,
      utils,
      rust-overlay,
      fenix,
    }:
    utils.lib.eachDefaultSystem (
      system:
      let
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
        nix-btm =
          with pkgs;
          rustPlatform.buildRustPackage {
            pname = "nix-btm";
            version = "0.3.0";

            src = ./.;
            # TODO cli flags to decide if we're client mode or daemon mode. This way we only build the client
            buildAndTestSubdir = "./crates/client";

            doCheck = false;

            cargoLock = {
              lockFile = ./Cargo.lock;
              # outputHashes = {
              #   "tui-tree-widget-0.20.0" = "sha256-wXAAR1IBeSpAZyD2OIr+Yt+8QoZNjYecXrv5I+7MoFw=";
              #
              # };
            };

            buildInputs = pkgs.lib.optionals pkgs.stdenv.isDarwin [
              pkgs.darwin.apple_sdk.frameworks.CoreServices
              pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
            ];

            meta = with lib; {
              description = "Rust tool to monitor nix processes";
              homepage = "https://github.com/DieracDelta/nix-btm";
              license = licenses.mit;
              mainProgram = "nix-btm";
            };
          };
      in
      {
        defaultPackage = nix-btm;
        packages.nix-btm = nix-btm;
        devShells.console = pkgs.mkShell.override { } {
          hardeningDisable = [ "fortify" ];
          RUSTFLAGS = "-C target-feature=+crt-static --cfg tokio_unstable";
          shellHook = ''
            export CARGO_TARGET_DIR="$(git rev-parse --show-toplevel)/target_dirs/nix_rustc";
          '';
          TOKIO_CONSOLE_BIND = "127.0.0.1:6669";
          TOKIO_CONSOLE_RETENTION = "60s";
          TOKIO_CONSOLE_BUFFER_CAPACITY = "2048";
          RUST_LOG = "info,tokio=trace,runtime=trace";
          RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
          buildInputs = with pkgs; [
            python3
            (rust-bin.stable.latest.minimal.override {
              extensions = [
                "cargo"
                "clippy"
                "rust-src"
                "rustc"
                "llvm-tools-preview"
              ];
              targets = [ "${arch}-unknown-${os}-musl" ];
            })
            (rust-bin.nightly.latest.minimal.override {
              extensions = [ "rustfmt" ];
              targets = [ "${arch}-unknown-${os}-musl" ];
            })

            just
            libiconv
            cargo-generate
            treefmt
            fenix.packages.${system}.rust-analyzer
            tokio-console
          ];
        };
        devShell = pkgs.mkShell.override { } {
          hardeningDisable = [ "fortify" ];
          RUSTFLAGS = "-C target-feature=+crt-static --cfg tokio_unstable";
          shellHook = ''
            export CARGO_TARGET_DIR="$(git rev-parse --show-toplevel)/target_dirs/nix_rustc";
          '';
          RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
          buildInputs = with pkgs; [
            python3
            (rust-bin.stable.latest.minimal.override {
              extensions = [
                "cargo"
                "clippy"
                "rust-src"
                "rustc"
                "llvm-tools-preview"
              ];
              targets = [ "${arch}-unknown-${os}-musl" ];
            })
            (rust-bin.nightly.latest.minimal.override {
              extensions = [ "rustfmt" ];
              targets = [ "${arch}-unknown-${os}-musl" ];
            })

            just
            libiconv
            cargo-generate
            treefmt
            fenix.packages.${system}.rust-analyzer
          ];
        };
      }
    );
}
