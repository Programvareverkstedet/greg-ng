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

    logLevel = lib.mkOption {
      type = lib.types.enum [ "quiet" "error" "warn" "info" "debug" "trace" ];
      default = "debug";
      description = "Log level.";
      apply = level: {
        "quiet" = "-q";
        "error" = "";
        "warn" = "-v";
        "info" = "-vv";
        "debug" = "-vvv";
        "trace" = "-vvvv";
      }.${level};
    };

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
      systemd.user.services.greg-ng = {
        description = "greg-ng, an mpv based media player";
        wantedBy = [ "graphical-session.target" ];
        partOf = [ "graphical-session.target" ];
        serviceConfig = {
          Type = "notify";
          ExecStart = let
            args = lib.cli.toGNUCommandLineShell { } (cfg.settings // {
              systemd = true;
            });
          in "${lib.getExe cfg.package} ${cfg.logLevel} ${args}";

          Restart = "always";
          RestartSec = 3;
          WatchdogSec = lib.mkDefault 15;
          TimeoutStartSec = lib.mkDefault 30;

          RestrictAddressFamilies = [ "AF_UNIX" "AF_INET" "AF_INET6" ];
          AmbientCapabilities = [ "" ];
          CapabilityBoundingSet = [ "" ];
          DeviceAllow = [ "" ];
          LockPersonality = true;
          # Might work, but wouldn't bet on it with embedded lua in mpv
          MemoryDenyWriteExecute = false;
          NoNewPrivileges = true;
          # MPV and mesa tries to talk directly to the GPU.
          PrivateDevices = false;
          PrivateMounts = true;
          PrivateTmp = true;
          PrivateUsers = true;
          ProcSubset = "pid";
          ProtectClock = true;
          ProtectControlGroups = true;
          # MPV wants ~/.cache
          ProtectHome = false;
          ProtectHostname = true;
          ProtectKernelLogs = true;
          ProtectKernelModules = true;
          ProtectKernelTunables = true;
          ProtectProc = "invisible";
          # I'll figure it out sometime
          # ProtectSystem = "full";
          RemoveIPC = true;
          UMask = "0077";
          RestrictNamespaces = true;
          RestrictRealtime = true;
          RestrictSUIDSGID = true;
          SystemCallArchitectures = "native";
          # Something brokey
          # SystemCallFilter = [
          #   "@system-service"
          #   "~@privileged"
          #   "~@resources"
          # ];
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

      users = {
        users.greg = {
          isNormalUser = true;
          group = "greg";
          uid = 2000;
          description = "loud gym bro";
        };
        groups.greg.gid = 2000;
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
