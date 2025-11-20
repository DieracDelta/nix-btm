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

pub mod double_fork;

// RPC control plane for client-daemon communication
pub mod rpc;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod rpc_client;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod rpc_daemon;
