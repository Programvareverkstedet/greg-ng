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
        format = pkgs.formats.toml { };
        configFile = format.generate "greg-ng.toml" {
          server.verbosity = "trace";
          mpv.auto_start = true;
        };
      in {
        type = "app";
        program = toString (pkgs.writeShellScript "greg-ng" ''
          exec ${lib.getExe package} --config ${configFile}
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
        inherit (self.packages.${prev.stdenv.hostPlatform.system}) greg-ng;
      };
    };

    packages = forAllSystems (system: pkgs: _: let
      inherit (self) sourceInfo;
      commitHash = sourceInfo.rev or (lib.substring 0 40 sourceInfo.dirtyRev);
      commitDate = "${lib.substring 0 4 sourceInfo.lastModifiedDate}-${lib.substring 4 2 sourceInfo.lastModifiedDate}-${lib.substring 6 2 sourceInfo.lastModifiedDate}";
      commitIsDirty = sourceInfo ? dirtyRev;
    in {
      default = self.packages.${system}.greg-ng;
      greg-ng = pkgs.callPackage ./default.nix {
        inherit commitHash commitDate commitIsDirty;
      };
      greg-ng-wrapped = pkgs.callPackage ./default.nix {
        wrapped = true;
        inherit commitHash commitDate commitIsDirty;
      };
    });
  } // {
    nixosModules.default = ./module.nix;
  };
}
