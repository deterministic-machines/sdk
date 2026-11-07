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
      in
      {
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
          ];
        };
      }
    );
}
