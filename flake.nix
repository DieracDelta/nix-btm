{
  description = "Nix Btm";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    utils.url = "github:numtide/flake-utils";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs@{ self, nixpkgs, utils, fenix}:
    utils.lib.eachDefaultSystem (system:
    let
        fenixStable = with fenix.packages.${system}; combine [
            (stable.withComponents [ "cargo" "clippy" "rust-src" "rustc" "rustfmt" "llvm-tools-preview" ])
          ];
        pkgs = import nixpkgs {
          inherit system;
          config = {
            allowUnfree = true;
          };
        };
        nix-btm =
with pkgs;
            rustPlatform.buildRustPackage {
              pname = "nix-btm";
              version = "0.1.0";

              src = ./.;

              cargoLock = {
                lockFile = ./Cargo.lock;
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
        in {
          defaultPackage = nix-btm;
          packages.nix-btm = nix-btm;
          devShell = pkgs.mkShell.override { } {
            shellHook = ''
              # export CARGO_TARGET_DIR="$(git rev-parse --show-toplevel)/target_ditrs/nix_rustc";
            '';
            RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
            buildInputs =
              with pkgs; [
              python3
                fenixStable
                fenix.packages.${system}.rust-analyzer
                just
                libiconv
                cargo-generate
                ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
                  pkgs.darwin.apple_sdk.frameworks.CoreServices
                  pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
                ];
          };
    });
}
