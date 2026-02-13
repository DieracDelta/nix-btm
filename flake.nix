{
  description = "nix-analytics: Build monitoring and control for Nix";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    devshell = {
      url = "github:numtide/devshell";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay, fenix, devshell, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system);
    in
    {
      nixosModules.default = import ./module.nix;

      packages = forAllSystems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };

          nixDev = pkgs.nix.dev;

          nixDevClosure = pkgs.closureInfo { rootPaths = [ nixDev ]; };
          nixPkgConfigDir = pkgs.runCommand "nix-dev-pkgconfig" {} ''
            mkdir -p $out/lib/pkgconfig
            while read storePath; do
              for d in "$storePath/lib/pkgconfig" "$storePath/share/pkgconfig"; do
                if [ -d "$d" ]; then
                  for f in "$d"/*.pc; do
                    [ -f "$f" ] && ln -sf "$f" "$out/lib/pkgconfig/"
                  done
                fi
              done
            done < ${nixDevClosure}/store-paths
          '';
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "nix-analytics";
            version = "0.1.0";
            src = pkgs.lib.cleanSource ./.;
            cargoLock.lockFile = ./Cargo.lock;
          };

          nix-analytics-plugin = pkgs.stdenv.mkDerivation {
            pname = "nix-analytics-plugin";
            version = "0.1.0";
            src = ./plugin;

            nativeBuildInputs = [
              pkgs.meson
              pkgs.ninja
              pkgs.pkg-config
            ];

            buildInputs = [
              nixDev
              pkgs.boost
            ];

            PKG_CONFIG_PATH = "${nixPkgConfigDir}/lib/pkgconfig";
          };
        }
      );

      checks = forAllSystems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
        in
        {
          rust-tests = pkgs.rustPlatform.buildRustPackage {
            pname = "nix-analytics-tests";
            version = "0.1.0";
            src = pkgs.lib.cleanSource ./.;
            cargoLock.lockFile = ./Cargo.lock;
            doCheck = true;
          };

          vm-e2e = import ./tests/vm-test.nix { inherit self pkgs; };
        }
      );

      apps = forAllSystems (system:
        let
          vm = import ./dev-vm.nix {
            inherit self nixpkgs system;
            projectDir = builtins.getEnv "PWD";
          };
        in
        {
          dev-vm = {
            type = "app";
            program = "${vm}/bin/run-nixos-vm";
          };
        }
      );

      devShells = forAllSystems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [
              rust-overlay.overlays.default
              devshell.overlays.default
            ];
          };

          fenixPkgs = fenix.packages.${system};

          nixDev = pkgs.nix.dev;

          # Collect all pkg-config paths from the nix.dev closure automatically.
          nixDevClosure = pkgs.closureInfo { rootPaths = [ nixDev ]; };
          nixPkgConfigDir = pkgs.runCommand "nix-dev-pkgconfig" {} ''
            mkdir -p $out/lib/pkgconfig
            while read storePath; do
              for d in "$storePath/lib/pkgconfig" "$storePath/share/pkgconfig"; do
                if [ -d "$d" ]; then
                  for f in "$d"/*.pc; do
                    [ -f "$f" ] && ln -sf "$f" "$out/lib/pkgconfig/"
                  done
                fi
              done
            done < ${nixDevClosure}/store-paths
          '';

          rustToolchain = pkgs.rust-bin.stable.latest.default.override {
            extensions = [ "rust-src" "clippy" "rustfmt" ];
          };
        in
        {
          default = pkgs.devshell.mkShell {
            name = "nix-analytics";

            packages = [
              # Rust toolchain (oxalica)
              rustToolchain

              # rust-analyzer (fenix)
              fenixPkgs.rust-analyzer

              # C++ plugin build deps
              pkgs.meson
              pkgs.ninja
              pkgs.pkg-config
              nixDev

              # Dev tools
              pkgs.cargo-watch
            ];

            env = [
              {
                name = "RUST_SRC_PATH";
                value = "${rustToolchain}/lib/rustlib/src/rust/library";
              }
              {
                name = "PKG_CONFIG_PATH";
                value = "${nixPkgConfigDir}/lib/pkgconfig";
              }
            ];

            commands = [
              {
                name = "check";
                help = "Run cargo check on the workspace";
                command = "cargo check --workspace";
              }
              {
                name = "build-plugin";
                help = "Build the C++ plugin with meson";
                command = "cd plugin && meson setup builddir --wipe 2>/dev/null; meson setup builddir 2>/dev/null || true && ninja -C builddir";
              }
            ];
          };
        }
      );
    };
}
