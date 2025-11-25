use tokio::task::JoinHandle;

pub mod derivation_tree;
pub mod handle_internal_json;

// Platform-specific notification system (io_uring on Linux, kqueue on macOS)
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod notify;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod ring_reader;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod ring_writer;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod client_side;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod daemon_side;

pub mod protocol_common;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod protocol_testing;

// RPC control plane for client-daemon communication
pub mod rpc;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod rpc_client;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod rpc_daemon;

use tracing::Instrument;

#[allow(unexpected_cfgs)]
pub fn spawn_named<F>(name: &str, fut: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let span = tracing::info_span!("task", task_name = %name);

    let fut = fut.instrument(span);
    use futures::FutureExt;

    #[cfg(tokio_unstable)]
    {
        tokio::task::Builder::new()
            .name(name)
            .spawn(fut.boxed())
            .expect("failed to spawn task")
    }

    #[cfg(not(tokio_unstable))]
    {
        tokio::spawn(fut.boxed())
    }
}
