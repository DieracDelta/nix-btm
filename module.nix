{ config, lib, pkgs, ... }:

let
  cfg = config.services.nix-analytics;
in
{
  options.services.nix-analytics = {
    enable = lib.mkEnableOption "nix-analytics build monitoring";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.nix-analytics or (throw "nix-analytics package not found; add the flake overlay or set services.nix-analytics.package");
      description = "The nix-analytics Rust package (provides nix-analyticsd and nix-analytics binaries).";
    };

    pluginPackage = lib.mkOption {
      type = lib.types.package;
      default = pkgs.nix-analytics-plugin or (throw "nix-analytics-plugin package not found; add the flake overlay or set services.nix-analytics.pluginPackage");
      description = "The nix-analytics C++ plugin package (provides libnix-analytics.so).";
    };

    socketDir = lib.mkOption {
      type = lib.types.str;
      default = "/run/nix-analytics";
      description = ''
        Directory for the daemon's Unix sockets (events.sock and control.sock).
        Override this for rootless nix or custom deployments. The daemon, TUI, and
        ctl tool all respect the NIX_ANALYTICS_SOCKET_DIR environment variable,
        which this option sets.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    nix.settings = {
      plugin-files = [ "${cfg.pluginPackage}/lib/libnix-analytics.so" ];
      use-cgroups = true;
    };

    # Tell the plugin where to send events when using a custom socket dir.
    nix.extraOptions = lib.mkIf (cfg.socketDir != "/run/nix-analytics") ''
      analytics-socket = ${cfg.socketDir}/events.sock
    '';

    systemd.tmpfiles.rules = [
      "d ${cfg.socketDir} 0755 root root -"
    ];

    systemd.services.nix-analyticsd = {
      description = "Nix Analytics Daemon";
      after = [ "nix-daemon.service" ];
      wantedBy = [ "multi-user.target" ];

      environment.NIX_ANALYTICS_SOCKET_DIR = cfg.socketDir;

      serviceConfig = {
        ExecStart = "${cfg.package}/bin/nix-analyticsd";
        Restart = "always";
        RestartSec = 2;
      };
    };

    # Set the env var system-wide so TUI and ctl find the sockets.
    environment.sessionVariables.NIX_ANALYTICS_SOCKET_DIR = cfg.socketDir;
    environment.systemPackages = [ cfg.package ];
  };
}
