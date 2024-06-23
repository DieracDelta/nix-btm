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
        in {
          defaultPackage = self.devShell.${system};
          devShell = pkgs.mkShell.override { } {
            shellHook = ''
              # export CARGO_TARGET_DIR="$(git rev-parse --show-toplevel)/target_ditrs/nix_rustc";
            '';
            RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
            buildInputs =
              with pkgs; [
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
