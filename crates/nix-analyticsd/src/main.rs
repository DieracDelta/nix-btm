use nix_analyticsd::{cgroup, control, event_listener, machines, state};

use anyhow::Result;
use tracing_subscriber::EnvFilter;

use nix_analytics_common::protocol;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let event_socket = protocol::event_socket_path();
    let control_socket = protocol::control_socket_path();

    tracing::info!(
        ?event_socket,
        ?control_socket,
        "nix-analyticsd starting"
    );

    // Ensure the socket directory exists.
    if let Some(parent) = event_socket.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let state = state::SharedState::new();

    // Spawn the event listener (reads from plugin socket)
    let event_state = state.clone();
    let event_handle = tokio::spawn(async move {
        if let Err(e) = event_listener::run(&event_socket, event_state).await {
            tracing::error!("event listener failed: {e}");
        }
    });

    // Spawn the control server (serves TUI queries)
    let control_state = state.clone();
    let control_handle = tokio::spawn(async move {
        if let Err(e) = control::run(&control_socket, control_state).await {
            tracing::error!("control server failed: {e}");
        }
    });

    // Spawn the cgroup poller
    let cgroup_state = state.clone();
    let cgroup_handle = tokio::spawn(async move {
        cgroup::poll_loop(cgroup_state).await;
    });

    // Spawn the machine status poller
    let machine_state = state.clone();
    let machine_handle = tokio::spawn(async move {
        machines::poll_loop(machine_state).await;
    });

    tokio::select! {
        r = event_handle => { r?; }
        r = control_handle => { r?; }
        r = cgroup_handle => { r?; }
        r = machine_handle => { r?; }
    }

    Ok(())
}
