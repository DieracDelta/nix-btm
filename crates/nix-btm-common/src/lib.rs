pub mod derivation_tree;
pub mod handle_internal_json;

// right now the ipc is only done on linux
// in the future we want to add stuff for macos and remote builders using rpc
#[cfg(target_os = "linux")]
pub mod ring_reader;

#[cfg(target_os = "linux")]
pub mod ring_writer;

#[cfg(target_os = "linux")]
pub mod client_side;
#[cfg(target_os = "linux")]
pub mod daemon_side;

pub mod protocol_common;
#[cfg(target_os = "linux")]
pub mod protocol_testing;

pub mod double_fork;

// RPC control plane for client-daemon communication
pub mod rpc;

#[cfg(target_os = "linux")]
pub mod rpc_client;

#[cfg(target_os = "linux")]
pub mod rpc_daemon;
