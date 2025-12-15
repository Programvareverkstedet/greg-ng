{
  lib
, fetchFromGitHub
, rustPlatform
, makeWrapper
, mpv
, wrapped ? false
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

  nativeBuildInputs = [ makeWrapper ];

  cargoLock = {
    lockFile = ./Cargo.lock;
    outputHashes = {
      "mpvipc-async-0.1.0" = "sha256-f3TwX+u0IwSSET4u6cFB+/LOO9UclpY3a8JyWy6zWzo=";
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
