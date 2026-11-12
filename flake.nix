{
  description = "dvm.run's SDKs";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    { nixpkgs, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        sandboxServer = pkgs.pkgsStatic.rustPlatform.buildRustPackage {
          pname = "sandbox-server";
          version = (pkgs.lib.importTOML ./rust/sandbox-server/Cargo.toml).package.version;
          src = pkgs.lib.fileset.toSource {
            root = ./rust;
            fileset = pkgs.lib.fileset.unions [
              ./rust/Cargo.toml
              ./rust/Cargo.lock
              ./rust/dvm-api
              ./rust/dvm-run
              ./rust/sandbox-server
            ];
          };
          cargoLock.lockFile = ./rust/Cargo.lock;
          cargoBuildFlags = [ "--package" "sandbox-server" ];
          preBuild = "unset NIX_CFLAGS_LINK";
          doCheck = false;
          meta.platforms = [ "x86_64-linux" ];
        };
      in
      {
        packages = pkgs.lib.optionalAttrs (system == "x86_64-linux") {
          sandbox-server = sandboxServer;
          default = sandboxServer;
        };
        devShells.default = pkgs.mkShell {
          packages = [
            # The Python SDK's tests, and its build for PyPI.
            (pkgs.python3.withPackages (packages: [
              packages.httpx
              packages.pytest
              packages.build
            ]))
            # The JS SDK's tests run its TypeScript as it is, and its build.
            pkgs.nodejs_24
            # The Rust SDK's.
            pkgs.cargo
            pkgs.rustc
            pkgs.clippy
            pkgs.rustfmt
          ];
        };
      }
    );
}
