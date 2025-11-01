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
    { self
    , nixpkgs
    , rust-overlay
    , fenix
    ,
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
          map
            (system: {
              name = system;
              value = f system;
            })
            systems
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
              };

              # Make tokio::task::Builder available
              RUSTFLAGS = "--cfg tokio_unstable";

              buildInputs = pkgs.lib.optionals pkgs.stdenv.isDarwin [
                pkgs.darwin.apple_sdk.frameworks.CoreServices
                pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
              ];

              meta = with lib; {
                description = "Rust tool to monitor nix processes";
                homepage = "https://github.com/DieracDelta/nix-btm";
                license = licenses.gpl3;
                mainProgram = "nix-btm";
              };
            };

          consoleShell = pkgs.mkShell.override { } {
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
        in
        {
          packages = {
            default = nix-btm;
            nix-btm = nix-btm;
          };
          devShells = {
            default = devShell;
            console = consoleShell;
          };
        };

      # Build the big outputs tree from the per-system pieces
      all = forAllSystems perSystem;
    in
    {
      # Per-system package sets
      packages = builtins.mapAttrs (_: v: v.packages) all;

      # Per-system dev shells
      devShells = builtins.mapAttrs (_: v: v.devShells) all;

      # Keep a defaultPackage attrset for convenience (one per system)
      defaultPackage = builtins.mapAttrs (_: v: v.packages.default) all;
    };
}
