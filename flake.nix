{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    fenix.url = "github:nix-community/fenix";
    fenix.inputs.nixpkgs.follows = "nixpkgs";
  };


  outputs = { self, nixpkgs, fenix }@inputs:
  let 
    systems = [
      "x86_64-linux"
      "aarch64-linux"
      "aarch64-darwin"
    ];
    forAllSystems = f: nixpkgs.lib.genAttrs systems (system: let
      toolchain = fenix.packages.${system}.complete;
      pkgs = import nixpkgs {
        inherit system;
        overlays = [
          (_: super: let pkgs = fenix.inputs.nixpkgs.legacyPackages.${system}; in fenix.overlays.default pkgs pkgs)
        ];
      };
    in f system pkgs toolchain);
  in {
    devShell = forAllSystems (system: pkgs: toolchain: pkgs.mkShell {
      packages = [
        (toolchain.withComponents [
          "cargo" "rustc" "rustfmt" "clippy"
        ])
        pkgs.mpv
      ];
      RUST_SRC_PATH = "${toolchain.rust-src}/lib/rustlib/src/rust/";
    });
  };
}
