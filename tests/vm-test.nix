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

  # Node with a custom socket directory to test NIX_ANALYTICS_SOCKET_DIR.
  nodes.custom_socket = { config, pkgs, ... }: {
    imports = [ self.nixosModules.default ];

    services.nix-analytics = {
      enable = true;
      package = self.packages.${system}.default;
      pluginPackage = self.packages.${system}.nix-analytics-plugin;
      socketDir = "/run/custom-analytics";
    };

    virtualisation = {
      memorySize = 2048;
      cores = 2;
    };

    nix.settings.experimental-features = [ "nix-command" ];
  };

  testScript = ''
    import json
    import time

    # ── Scenario A: Standard multi-user NixOS (default socket dir) ──

    machine.wait_for_unit("nix-daemon.service")
    machine.wait_for_unit("nix-analyticsd.service")
    machine.wait_for_file("/run/nix-analytics/control.sock")

    # Verify empty state.
    result = machine.succeed("nix-analytics-ctl list-builds")
    data = json.loads(result)
    assert data["status"] == "Builds", f"expected Builds status, got {data}"
    assert data["builds"] == [], f"expected no builds, got {data['builds']}"

    # Trigger a trivial build.
    machine.succeed(
        "nix build --impure --expr 'derivation { name = \"test\"; system = \"${system}\"; builder = \"/bin/sh\"; args = [\"-c\" \"echo hello > $out\"]; }' 2>&1 || true"
    )

    time.sleep(2)

    # Query history — should have at least 1 entry.
    result = machine.succeed("nix-analytics-ctl get-history")
    data = json.loads(result)
    assert data["status"] == "History", f"expected History status, got {data}"
    assert len(data["builds"]) >= 1, f"expected at least 1 history entry, got {len(data['builds'])}"

    # Verify that user_uid is populated (builds go through nix-daemon which
    # creates cgroups named nix-build-uid-<UID>).
    build = data["builds"][0]["build"]
    # user_uid should be set from the cgroup dir name.
    assert build.get("user_uid") is not None, f"expected user_uid to be set, got build: {build}"

    # ── Scenario B: Custom socket directory ──

    custom_socket.wait_for_unit("nix-daemon.service")
    custom_socket.wait_for_unit("nix-analyticsd.service")
    custom_socket.wait_for_file("/run/custom-analytics/control.sock")

    # The default /run/nix-analytics should NOT exist on this node.
    custom_socket.succeed("test ! -e /run/nix-analytics/control.sock")

    # Verify ctl works with the custom socket dir (env var is set system-wide).
    result = custom_socket.succeed("nix-analytics-ctl list-builds")
    data = json.loads(result)
    assert data["status"] == "Builds", f"custom_socket: expected Builds, got {data}"

    # Run a build on the custom-socket node.
    custom_socket.succeed(
        "nix build --impure --expr 'derivation { name = \"test-custom\"; system = \"${system}\"; builder = \"/bin/sh\"; args = [\"-c\" \"echo hello > $out\"]; }' 2>&1 || true"
    )

    time.sleep(2)

    result = custom_socket.succeed("nix-analytics-ctl get-history")
    data = json.loads(result)
    assert data["status"] == "History", f"custom_socket: expected History, got {data}"
    assert len(data["builds"]) >= 1, f"custom_socket: expected at least 1 history entry, got {len(data['builds'])}"
  '';
}
