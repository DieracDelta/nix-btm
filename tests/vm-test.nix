{ self, pkgs
, analyticsPackage ? self.packages.${pkgs.system}.default
, pluginPackage ? self.packages.${pkgs.system}.nix-analytics-plugin
, nixVersionLabel ? "default"
}:

let
  system = pkgs.system;
  testSuffix = if nixVersionLabel == "default" then "" else "-nix-${nixVersionLabel}";
in
pkgs.testers.runNixOSTest {
  name = "nix-analytics-e2e${testSuffix}";

  nodes.machine = { config, pkgs, ... }: {
    imports = [ self.nixosModules.default ];

    services.nix-analytics = {
      enable = true;
      package = analyticsPackage;
      pluginPackage = pluginPackage;
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
      package = analyticsPackage;
      pluginPackage = pluginPackage;
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

    crash_patterns = [
        "SIGABRT", "SIGSEGV", "double free", "corrupted",
        "core dumped", "segfault",
    ]

    def check_no_crashes(node, name):
        journal = node.succeed("journalctl --no-pager -b")
        for pat in crash_patterns:
            if pat.lower() in journal.lower():
                # Grab surrounding context
                lines = journal.splitlines()
                matches = [l for l in lines if pat.lower() in l.lower()]
                context = "\n".join(matches[:10])
                raise Exception(
                    f"Crash indicator '{pat}' found on {name}:\n{context}"
                )

    # ── Scenario A: Standard multi-user NixOS (default socket dir) ──

    machine.wait_for_unit("nix-daemon.socket")
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
    check_no_crashes(machine, "machine")

    # Query history — should have at least 1 entry.
    result = machine.succeed("nix-analytics-ctl get-history")
    data = json.loads(result)
    assert data["status"] == "History", f"expected History status, got {data}"
    assert len(data["builds"]) >= 1, f"expected at least 1 history entry, got {len(data['builds'])}"

    # Check that Build-type history entries have user_uid populated (builds go
    # through nix-daemon which creates cgroups named nix-build-uid-<UID>).
    # FileTransfer and other activity types won't have cgroups.
    build_entries = [b for b in data["builds"] if b["build"]["activity_type"] == "Build"]
    for entry in build_entries:
        build = entry["build"]
        # user_uid should be set from the cgroup dir name when cgroups are discovered.
        # For very fast builds the cgroup poller may not have time to assign, so only
        # assert if cgroup_path was actually discovered.
        if build.get("cgroup_path") is not None:
            assert build.get("user_uid") is not None, f"cgroup assigned but user_uid missing: {build}"

    # ── Scenario B: Custom socket directory ──

    custom_socket.wait_for_unit("nix-daemon.socket")
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

    check_no_crashes(custom_socket, "custom_socket")
  '';
}
