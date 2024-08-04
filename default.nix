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
  src = lib.cleanSource ./.;

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
