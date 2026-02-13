{ self, pkgs }:

let
  system = pkgs.system;
in
pkgs.testers.runNixOSTest {
  name = "nix-analytics-e2e";

  nodes.machine = { config, pkgs, ... }: {
    imports = [ self.nixosModules.default ];

    services.nix-analytics = {
      enable = true;
      package = self.packages.${system}.default;
      pluginPackage = self.packages.${system}.nix-analytics-plugin;
    };

    virtualisation = {
      memorySize = 2048;
      cores = 2;
    };

    nix.settings.experimental-features = [ "nix-command" ];
  };

  testScript = ''
    machine.wait_for_unit("nix-daemon.service")
    machine.wait_for_unit("nix-analyticsd.service")
    machine.wait_for_file("/run/nix-analytics/control.sock")

    # Verify empty state.
    result = machine.succeed("nix-analytics-ctl list-builds")
    import json
    data = json.loads(result)
    assert data["status"] == "Builds", f"expected Builds status, got {data}"
    assert data["builds"] == [], f"expected no builds, got {data['builds']}"

    # Trigger a trivial build.
    machine.succeed(
        "nix build --impure --expr 'derivation { name = \"test\"; system = \"${system}\"; builder = \"/bin/sh\"; args = [\"-c\" \"echo hello > $out\"]; }' 2>&1 || true"
    )

    import time
    time.sleep(2)

    # Query history — should have at least 1 entry.
    result = machine.succeed("nix-analytics-ctl get-history")
    data = json.loads(result)
    assert data["status"] == "History", f"expected History status, got {data}"
    assert len(data["builds"]) >= 1, f"expected at least 1 history entry, got {len(data['builds'])}"
  '';
}
