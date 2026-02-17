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

    nix-2-33 = {
      url = "github:NixOS/nix/2.33.0";
    };
  };

  outputs = { self, nixpkgs, rust-overlay, fenix, devshell, nix-2-33, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system);
    in
    {
      nixosModules.default = import ./module.nix;

      overlays.default = final: prev: {
        nix-analytics = final.rustPlatform.buildRustPackage {
          pname = "nix-analytics";
          version = "0.1.0";
          src = final.lib.cleanSource ./.;
          cargoLock.lockFile = ./Cargo.lock;
          GIT_HASH = self.shortRev or self.dirtyShortRev or "dev";
        };

        nix-analytics-plugin = let
          nixDev = final.nix.dev;
          nixDevClosure = final.closureInfo { rootPaths = [ nixDev ]; };
          nixPkgConfigDir = final.runCommand "nix-dev-pkgconfig" {} ''
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
        in final.stdenv.mkDerivation {
          pname = "nix-analytics-plugin";
          version = "0.1.0";
          src = ./plugin;
          nativeBuildInputs = [ final.meson final.ninja final.pkg-config ];
          buildInputs = [ nixDev final.boost ];
          PKG_CONFIG_PATH = "${nixPkgConfigDir}/lib/pkgconfig";
        };
      };

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
            GIT_HASH = self.shortRev or self.dirtyShortRev or "dev";
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

          mkVersionedTest = nixFlakeInput: versionLabel:
            let
              nixPkg = nixFlakeInput.packages.${system}.nix;
              versionedPkgs = import nixpkgs {
                inherit system;
                overlays = [
                  rust-overlay.overlays.default
                  (final: prev: { nix = nixPkg; })
                  self.overlays.default
                ];
              };
            in import ./tests/vm-test.nix {
              inherit self;
              pkgs = versionedPkgs;
              analyticsPackage = versionedPkgs.nix-analytics;
              pluginPackage = versionedPkgs.nix-analytics-plugin;
              nixVersionLabel = versionLabel;
            };
        in
        {
          rust-tests = pkgs.rustPlatform.buildRustPackage {
            pname = "nix-analytics-tests";
            version = "0.1.0";
            src = pkgs.lib.cleanSource ./.;
            cargoLock.lockFile = ./Cargo.lock;
            doCheck = true;
            GIT_HASH = self.shortRev or self.dirtyShortRev or "dev";
          };

          vm-e2e = import ./tests/vm-test.nix { inherit self pkgs; };
          vm-e2e-nix-2-33 = mkVersionedTest nix-2-33 "2.33.0";
        }
      );

      apps = forAllSystems (system:
        let
          mkVm = scenario: import ./dev-vm.nix {
            inherit self nixpkgs system scenario;
            projectDir = builtins.getEnv "PWD";
          };
        in
        {
          dev-vm = {
            type = "app";
            program = "${mkVm "default"}/bin/run-nixos-vm";
          };
          dev-vm-root = {
            type = "app";
            program = "${mkVm "root"}/bin/run-nixos-vm";
          };
          dev-vm-daemonless = {
            type = "app";
            program = "${mkVm "daemonless"}/bin/run-nixos-vm";
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
