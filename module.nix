{ config, lib, pkgs, ... }:

let
  cfg = config.services.nix-btm;
in
{
  options.services.nix-btm = {
    enable = lib.mkEnableOption "nix-btm build monitoring";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.nix-btm or (throw "nix-btm package not found; add the flake overlay or set services.nix-btm.package");
      description = "The nix-btm Rust package (provides nix-btmd and nix-btm binaries).";
    };

    pluginPackage = lib.mkOption {
      type = lib.types.package;
      default = pkgs.nix-btm-plugin or (throw "nix-btm-plugin package not found; add the flake overlay or set services.nix-btm.pluginPackage");
      description = "The nix-btm C++ plugin package (provides libnix-btm.so).";
    };

    socketDir = lib.mkOption {
      type = lib.types.str;
      default = "/run/nix-btm";
      description = ''
        Directory for the daemon's Unix sockets (events.sock and control.sock).
        Override this for rootless nix or custom deployments. The daemon, TUI, and
        ctl tool all respect the NIX_BTM_SOCKET_DIR environment variable,
        which this option sets.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    nix.settings = {
      plugin-files = [ "${cfg.pluginPackage}/lib/libnix-btm.so" ];
      use-cgroups = true;
    };

    # Tell the plugin where to send events when using a custom socket dir.
    nix.extraOptions = lib.mkIf (cfg.socketDir != "/run/nix-btm") ''
      btm-socket = ${cfg.socketDir}/events.sock
    '';

    systemd.tmpfiles.rules = [
      "d ${cfg.socketDir} 0755 root root -"
    ];

    systemd.services.nix-btmd = {
      description = "Nix BTM Daemon";
      after = [ "nix-daemon.service" ];
      wantedBy = [ "multi-user.target" ];

      environment.NIX_BTM_SOCKET_DIR = cfg.socketDir;

      serviceConfig = {
        ExecStart = "${cfg.package}/bin/nix-btmd";
        Restart = "always";
        RestartSec = 2;
      };
    };

    # Set the env var system-wide so TUI and ctl find the sockets.
    environment.sessionVariables.NIX_BTM_SOCKET_DIR = cfg.socketDir;
    environment.systemPackages = [ cfg.package ];
  };
}
