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
      toolchain = rust-bin.stable.latest.default.override {
        extensions = [ "rust-src" "rust-analyzer" "rust-std" ];
      };
    in f system pkgs toolchain);
  in {
    apps = forAllSystems (system: pkgs: _: {
      default = self.apps.${system}.greg-ng;
      greg-ng = let
        package = self.packages.${system}.greg-ng-wrapped;
      in {
        type = "app";
        program = toString (pkgs.writeShellScript "greg-ng" ''
          ${lib.getExe package} --mpv-socket-path /tmp/greg-ng-mpv.sock -vvvv
        '');
      };
    });

    devShells = forAllSystems (system: pkgs: toolchain: {
      default = pkgs.mkShell {
        nativeBuildInputs = [
          toolchain
          pkgs.mpv
          pkgs.cargo-edit
        ];

        RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";
      };
    });

    overlays = {
      default = self.overlays.greg-ng;
      greg-ng = final: prev: {
        inherit (self.packages.${prev.system}) greg-ng;
      };
    };

    packages = forAllSystems (system: pkgs: _: {
      default = self.packages.${system}.greg-ng;
      greg-ng = pkgs.callPackage ./default.nix { };
      greg-ng-wrapped = pkgs.callPackage ./default.nix {
        wrapped = true;
      };
    });
  } // {
    nixosModules.default = ./module.nix;
  };
}
