{ config, pkgs, lib, ... }:
let
  cfg = config.services.greg-ng;
in
{
  options.services.greg-ng = {
    enable = lib.mkEnableOption "greg-ng, an mpv based media player";

    package = lib.mkPackageOption pkgs "greg-ng" { };

    mpvPackage = lib.mkPackageOption pkgs "mpv" { };

    enableSway = lib.mkEnableOption "sway as the main window manager";

    enablePipewire = lib.mkEnableOption "pipewire" // { default = true; };

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

  config = lib.mkMerge [
    (lib.mkIf cfg.enable {
      users = {
        users.greg = {
          isNormalUser = true;
          group = "greg";
          uid = 2000;
          description = "loud gym bro";
        };
        groups.greg.gid = 2000;
      };

      systemd.user.services.greg-ng = {
        description = "greg-ng, an mpv based media player";
        wantedBy = [ "graphical-session.target" ];
        partOf = [ "graphical-session.target" ];
        serviceConfig = {
          Type = "simple";
          ExecStart = "${lib.getExe cfg.package} ${lib.cli.toGNUCommandLineShell { } cfg.settings}";
          Restart = "always";
          RestartSec = 3;
        };
      };
    })
    (lib.mkIf (cfg.enable && cfg.enablePipewire) {
      services.pipewire = {
        enable = true;
        alsa.enable = true;
        alsa.support32Bit = true;
        pulse.enable = true;
      };
    })
    (lib.mkIf (cfg.enable && cfg.enableSway) {
      programs.sway = {
        enable = true;
        wrapperFeatures.gtk = true;
      };

      xdg.portal = {
        enable = true;
        wlr.enable = true;
        extraPortals = [ pkgs.xdg-desktop-portal-gtk ];
      };

      services.greetd = {
        enable = true;
        settings = rec {
          initial_session = {
            command = "${pkgs.sway}/bin/sway";
            user = "greg";
          };
          default_session = initial_session;
        };
      };
    })
  ];
}
