{ config, pkgs, lib, ... }:
let
  cfg = config.services.greg-ng;
in
{
  options.services.greg-ng = {
    enable = lib.mkEnableOption "greg-ng, an mpv based media player";

    package = lib.mkPackageOption pkgs "greg-ng" { };

    mpvPackage = lib.mkPackageOption pkgs "mpv" { };

    # TODO: create some better descriptions
    settings = {
      host = lib.mkOption {
        type = lib.types.str;
        default = "localhost";
        example = "0.0.0.0";
        description = ''
          Which host to bind to.
        '';
      };

      port = lib.mkOption {
        type = lib.types.port;
        default = 8008;
        example = 10008;
        description = ''
          Which port to bind to.
        '';
      };

      mpv-socket-path = lib.mkOption {
        type = lib.types.str;
        default = "%t/greg-ng-mpv.sock";
        description = ''
          Path to the mpv socket.
        '';
      };

      mpv-executable-path = lib.mkOption {
        type = lib.types.str;
        default = lib.getExe cfg.mpvPackage;
        defaultText = lib.literalExpression ''
          lib.getExe config.services.greg-ng.mpvPackage
        '';
        description = ''
          Path to the mpv executable.
        '';
      };

      mpv-config-file = lib.mkOption {
        type = with lib.types; nullOr str;
        default = null;
        description = ''
          Path to the mpv config file.
        '';
      };

      auto-start-mpv = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Whether to automatically start mpv.
        '';
      };

      force-auto-start = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Whether to force auto starting mpv.
        '';
      };
    };
  };

  config = {
    services.cage.enable = true;
    services.cage.program =let
      flags = lib.cli.toGNUCommandLineShell { } cfg.settings;
    in pkgs.writeShellScript "greg-kiosk" ''
      cd $(mktemp -d)

      ${lib.getExe cfg.package} ${flags}
    '';
    services.cage.user = "greg";
    users.users."greg".isNormalUser = true;
    system.activationScripts = {
      base-dirs = {
        text = ''
          mkdir -p /nix/var/nix/profiles/per-user/greg
        '';
        deps = [];
      };
    };
  };
}
