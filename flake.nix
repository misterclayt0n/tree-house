{
  description = "A package manager for tree-sitter grammars";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    # TODO: drop crane and only use rust-overlay? Can we just use
    # the rust helpers from nixpkgs and eliminate all other deps?
    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, crane, flake-utils, rust-overlay, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [(import rust-overlay)];
        };
        stdenv = if pkgs.stdenv.isLinux then pkgs.stdenv else pkgs.clangStdenv;
        rustFlagsEnv =
          if stdenv.isLinux
          then ''$RUSTFLAGS -C link-arg=-fuse-ld=lld -C target-cpu=native -Clink-arg=-Wl,--no-rosegment''
          else "$RUSTFLAGS";
        rustToolchain = pkgs.pkgsBuildHost.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;
        commonArgs =
          {
            src = ./.;
            buildInputs = [pkgs.stdenv.cc.cc.lib];
            doCheck = false;
          }
          // craneLib.crateNameFromCargoToml {cargoToml = ./cli/Cargo.toml;};
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;
      in {
        packages = {
          skidder-cli = craneLib.buildPackage (commonArgs // {
            inherit cargoArtifacts;
          });
          default = self.packages.${system}.skidder-cli;
        };

        overlays.default = final: prev: {
          inherit (self.packages.${system}) skidder-cli;
        };

        checks = {
          clippy = craneLib.cargoClippy (commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--all-targets -- --deny warnings";
            });

          fmt = craneLib.cargoFmt commonArgs;

          doc = craneLib.cargoDoc (commonArgs
            // {
              inherit cargoArtifacts;
            });

          test = craneLib.cargoTest (commonArgs
            // {
              inherit cargoArtifacts;
            });
        };

        devShells.default = pkgs.mkShell {
          RUST_BACKTRACE = "1";
          inputsFrom = builtins.attrValues self.checks.${system};
          nativeBuildInputs = with pkgs;
            [lld_13 cargo-flamegraph rust-analyzer]
              ++ (lib.optional (stdenv.isx86_64 && stdenv.isLinux) pkgs.cargo-tarpaulin)
              ++ (lib.optional stdenv.isLinux pkgs.lldb)
              ++ (lib.optional stdenv.isDarwin pkgs.darwin.apple_sdk.frameworks.CoreFoundation);
          shellHook = ''
            export RUSTFLAGS="${rustFlagsEnv}"
          '';
        };
      }
    );
}
