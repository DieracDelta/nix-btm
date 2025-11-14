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
              RUSTFLAGS = "-C target-feature=+crt-static --cfg tokio_unstable" + maybe_hardcoded_hack;
              CARGO_BUILD_TARGET = "x86_64-unknown-linux-musl";

              meta = with lib; {
                description = "Rust tool to monitor nix processes";
                homepage = "https://github.com/DieracDelta/nix-btm";
                license = licenses.gpl3;
                mainProgram = "nix-btm";
              };
            };

          consoleShell = pkgs.mkShell.override { } {
            hardeningDisable = [ "fortify" ];
            RUSTFLAGS = "-C target-feature=+crt-static --cfg tokio_unstable" + maybe_hardcoded_hack;
            CARGO_BUILD_TARGET = "x86_64-unknown-linux-musl";
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
              ++ rust_tc;
            # ++ maybe_libiconv;
          };

          devShell = pkgs.mkShell.override { } {
            hardeningDisable = [ "fortify" ];
            RUSTFLAGS = "-C target-feature=+crt-static --cfg tokio_unstable" + maybe_hardcoded_hack;
            CARGO_BUILD_TARGET = "x86_64-unknown-linux-musl";
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
              ++ rust_tc;
            # ++ maybe_libiconv;
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

      all = forAllSystems perSystem;
    in
    {
      packages = builtins.mapAttrs (_: v: v.packages) all;

      devShells = builtins.mapAttrs (_: v: v.devShells) all;

      defaultPackage = builtins.mapAttrs (_: v: v.packages.default) all;
    };
}
