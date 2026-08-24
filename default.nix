{
  lib
, fetchFromGitHub
, rustPlatform
, makeWrapper
, mpv
, wrapped ? false
, commitHash ? "unknown"
, commitDate ? "unknown"
, commitIsDirty ? false
}:

rustPlatform.buildRustPackage rec {
  pname = "greg-ng";
  version = "0.1.0";
  src = builtins.filterSource (path: type: let
    baseName = baseNameOf (toString path);
  in !(lib.any (b: b) [
      (!(lib.cleanSourceFilter path type))
      (type == "directory" && lib.elem baseName [
        ".direnv"
        ".git"
        "target"
        "result"
      ])
      (type == "regular" && lib.elem baseName [
        "flake.nix"
        "flake.lock"
        "default.nix"
        "module.nix"
        ".envrc"
      ])
    ])) ./.;

  env = {
    GIT_COMMIT = commitHash;
    GIT_COMMIT_DATE = commitDate;
    GIT_DIRTY = lib.boolToString commitIsDirty;
  };

  nativeBuildInputs = [ makeWrapper ];

  cargoLock = {
    lockFile = ./Cargo.lock;
    outputHashes = {
      "mpvipc-async-0.2.0" = "sha256-utU7M9hazXSoua4eHEnnnuYfhNSufpsTX8YOzGw0xPQ=";
    };
  };

  postInstall = lib.optionalString wrapped ''
    wrapProgram $out/bin/greg-ng \
      --prefix PATH : '${lib.makeBinPath [ mpv ]}'
  '';

  meta = with lib; {
    license = licenses.gpl3Only;
    maintainers = with maintainers; [ h7x4 ];
    platforms = platforms.linux ++ platforms.darwin;
    mainProgram = "greg-ng";
  };
}
