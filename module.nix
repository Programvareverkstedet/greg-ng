{ config, pkgs, lib, ... }:
let
  cfg = config.services.greg-ng;
  format = pkgs.formats.toml { };

  mpvFormat = pkgs.formats.ini {
    mkKeyValue = lib.generators.mkKeyValueDefault {
      mkValueString = v:
        if lib.isBool v then (if v then "yes" else "no")
        else lib.generators.mkValueStringDefault { } v;
    } "=";
  };
in
{
  options.services.greg-ng = {
    enable = lib.mkEnableOption "greg-ng, an mpv based media player";

    package = lib.mkPackageOption pkgs "greg-ng" { };

    mpv = {
      package = lib.mkPackageOption pkgs "mpv" { };

      settings = lib.mkOption {
        type = mpvFormat.type;
        default = { };
        example = {
          default = {
            hwdec = "vaapi";
            gpu-api = "opengl";
          };
          youtube = {
            profile-cond = "path:find('youtu%.?be')";
            ytdl-format = "bestvideo[height<=?720][vcodec^=avc1]+bestaudio/best";
          };
        };
        description = ''
          The INI configuration for mpv.

          See:
          - <https://mpv.io/manual/stable/#options> for available options
          - <https://mpv.io/manual/stable/#profiles> for info about profiles
        '';
      };
    };

    enableSway = lib.mkEnableOption "sway as the main window manager";

    enablePipewire = lib.mkEnableOption "pipewire" // { default = true; };

    # TODO: create some better descriptions
    settings = lib.mkOption {
      default = { };
      type = lib.types.submodule {
        freeformType = format.type;
        options = {
          server = {
            host = lib.mkOption {
              type = lib.types.str;
              default = "127.0.0.1";
              example = "0.0.0.0";
              description = ''
                Which IP address to bind to.
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

            verbosity = lib.mkOption {
              type = lib.types.enum [ "off" "error" "warn" "info" "debug" "trace" ];
              default = "debug";
              description = "Log level.";
            };
          };

          mpv = {
            socket_path = lib.mkOption {
              type = with lib.types; nullOr str;
              default = null;
              example = "%t/greg-ng-mpv.sock";
              description = ''
                Path to the mpv socket to connect to, when not auto-starting mpv.
              '';
            };

            executable_path = lib.mkOption {
              type = lib.types.str;
              default = lib.getExe cfg.mpv.package;
              defaultText = lib.literalExpression ''
                lib.getExe config.services.greg-ng.mpv.package
              '';
              description = ''
                Path to the mpv executable.
              '';
            };

            config_file = lib.mkOption {
              type = lib.types.str;
              readOnly = true;
              internal = true;
              description = ''
                Path to the mpv config file. Managed by the nix module,
                use {option}`services.greg-ng.mpv.settings` to configure mpv.
              '';
            };

            log_level = lib.mkOption {
              type = lib.types.enum [
                "no"
                "fatal"
                "error"
                "warn"
                "info"
                "status"
                "v"
                "debug"
                "trace"
              ];
              default = "info";
              example = "v";
              description = "The minimum level of mpv log messages to relay to the greg-ng log.";
            };

            auto_start = lib.mkOption {
              type = with lib.types; nullOr bool;
              default = true;
              description = ''
                Whether to automatically start our own private mpv instance.

                ::: {.note}
                This mpv instance will not use the socket path specified in
                {option}`services.greg-ng.settings.mpv.socket_path`.
                :::
              '';
            };
          };
        };
      };
    };
  };

  config = lib.mkMerge [
    (let
      mpvConfigFile = mpvFormat.generate "greg-ng-mpv.conf" (
        lib.filterAttrsRecursive (_: v: v != null) cfg.mpv.settings
      );
    in lib.mkIf cfg.enable {
      # See assets/default-mpv.conf
      services.greg-ng.mpv.settings.default = {
        volume-max = lib.mkDefault 100;
        hwdec = lib.mkDefault "auto-safe";
        cursor-autohide = lib.mkDefault "always";
      };

      environment.etc."greg-ng/mpv.conf".source = mpvConfigFile;

      services.greg-ng.settings.mpv.config_file = "/etc/greg-ng/mpv.conf";

      systemd.user.sockets.greg-ng = {
        description = "Socket for greg-ng, an mpv based media player";
        wantedBy = [ "sockets.target" ];
        socketConfig = {
          ListenStream = "${cfg.settings.server.host}:${toString cfg.settings.server.port}";
        };
      };

      systemd.user.services.greg-ng = {
        description = "greg-ng, an mpv based media player";
        wantedBy = [ "graphical-session.target" ];
        partOf = [ "graphical-session.target" ];
        restartTriggers = [ mpvConfigFile ];
        serviceConfig = {
          Type = "notify";
          ExecStart = let
            configFile = format.generate "greg-ng.toml" (
              lib.filterAttrsRecursive (_: v: v != null) cfg.settings
            );
          in "${lib.getExe cfg.package} --systemd --config ${configFile}";

          Sockets = [ "greg-ng.socket" ];

          CacheDirectory = "greg-ng";
          RuntimeDirectory = "greg-ng";

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
