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
  };

  config = lib.mkIf cfg.enable {
    nix.settings = {
      plugin-files = [ "${cfg.pluginPackage}/lib/libnix-analytics.so" ];
      use-cgroups = true;
    };

    systemd.tmpfiles.rules = [
      "d /run/nix-analytics 0755 root root -"
    ];

    systemd.services.nix-analyticsd = {
      description = "Nix Analytics Daemon";
      after = [ "nix-daemon.service" ];
      wantedBy = [ "multi-user.target" ];

      serviceConfig = {
        ExecStart = "${cfg.package}/bin/nix-analyticsd";
        Restart = "always";
        RestartSec = 2;
      };
    };

    environment.systemPackages = [ cfg.package ];
  };
}
