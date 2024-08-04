{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, rust-overlay }:
  let
    inherit (nixpkgs) lib;

    systems = [
      "x86_64-linux"
      "aarch64-linux"
      "x86_64-darwin"
      "aarch64-darwin"
      "armv7l-linux"
    ];

    forAllSystems = f: lib.genAttrs systems (system: let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [
          (import rust-overlay)
        ];
      };

      rust-bin = rust-overlay.lib.mkRustBin { } pkgs.buildPackages;
      toolchain = rust-bin.stable.latest.default;
    in f system pkgs toolchain);
  in {
    apps = forAllSystems (system: pkgs: _: {
      default = self.apps.${system}.greg-ng;
      greg-ng = let
        package = self.packages.${system}.greg-ng;
      in {
        type = "app";
        program = lib.getExe package;
      };
    });

    devShells = forAllSystems (system: pkgs: toolchain: {
      default = pkgs.mkShell {
        nativeBuildInputs = [
          toolchain
          pkgs.mpv
        ];

        RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";
      };
    });

    packages = forAllSystems (system: pkgs: _: {
      default = self.packages.${system}.greg-ng;
      greg-ng = pkgs.callPackage ./default.nix { };
    });
  };
}
