{
  lib
, fetchFromGitHub
, rustPlatform
, makeWrapper
, mpv
}:

rustPlatform.buildRustPackage rec {
  pname = "greg-ng";
  version = "0.1.0";
  src = builtins.filterSource (path: type: let
    baseName = baseNameOf (toString path);
  in !(lib.any (b: b) [
      (!(lib.cleanSourceFilter path type))
      (baseName == "target" && type == "directory")
      (baseName == "nix" && type == "directory")
      (baseName == "flake.nix" && type == "regular")
      (baseName == "flake.lock" && type == "regular")
    ])) ./.;


  nativeBuildInputs = [ makeWrapper ];

  cargoLock = {
    lockFile = ./Cargo.lock;
    outputHashes = {
      "mpvipc-async-0.1.0" = "sha256-2TQ2d4q9/DTxTZe9kOAoKBhsmegRZw32x3G2hbluS6U=";
    };
  };

  postInstall = ''
    wrapProgram $out/bin/greg-ng \
      --prefix PATH : '${lib.makeBinPath [ mpv ]}'
  '';

  meta = with lib; {
    license = licenses.mit;
    maintainers = with maintainers; [ h7x4 ];
    platforms = platforms.linux ++ platforms.darwin;
    mainProgram = "greg-ng";
  };
}
