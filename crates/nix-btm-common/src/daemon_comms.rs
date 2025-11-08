use std::{
    backtrace::Backtrace,
    ffi::CString,
    fmt::Debug,
    os::fd::{AsFd, OwnedFd},
};

use bytemuck::bytes_of;
use psx_shm::{Shm, UnlinkOnDrop};
pub use rustix::*;
use rustix::{
    fs::{MemfdFlags, Mode, ftruncate, memfd_create},
    mm::{MapFlags, ProtFlags, mmap},
    shm::OFlags,
};
use serde_cbor as cbor;
use snafu::{ResultExt, Snafu};
use tokio::net::UnixStream;

use crate::{
    client_daemon_comms::{JobsStateInnerWire, SnapshotHeader},
    handle_internal_json::JobsStateInner,
};

pub fn daemon_double_fork(socket_path: String, json_file_path: String) {
    todo!();
}

/// Align `x` up to the next multiple of 2^`align_pow` bytes.
#[inline]
pub const fn align_up_pow2(num_bytes: u64, align_pow: u32) -> u64 {
    let align = 1u64 << align_pow; // the actual alignment
    (num_bytes + (align - 1)/* round up */) & !(align - 1/* truncate down */)
}

// align to a multiple of the page size
#[inline]
pub fn round_up_page(num_bytes: u64) -> u64 {
    let num_bytes_page = rustix::param::page_size() as u64;
    // same reasoning as align_up_pow2
    (num_bytes + (num_bytes_page - 1)) & !(num_bytes_page - 1)
}

use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};

fn get_pid(stream: &UnixStream) -> Option<i32> {
    getsockopt(&stream.as_fd(), PeerCredentials)
        .ok()
        .map(|x| x.pid())
}

// the snapshot in memory
#[derive(Debug)]
pub struct SnapshotMemfd {
    pub shmem: UnlinkOnDrop,
    pub total_len_bytes: u64,
    pub snap_seq_uid: u64,
}

#[derive(Snafu, Debug)]
pub enum ProtocolError {
    #[snafu(display("I/O error: {source}"), visibility(pub(crate)))]
    Io {
        source: std::io::Error,
        #[snafu(backtrace)]
        backtrace: Backtrace,
    },
    //
    #[snafu(display("CBOR error: {source}"), visibility(pub(crate)))]
    Cbor {
        source: serde_cbor::Error,
        #[snafu(backtrace)]
        backtrace: Backtrace,
    },
    #[snafu(display("Mismatch Error"), visibility(pub(crate)))]
    MisMatchError {
        #[snafu(backtrace)]
        backtrace: Backtrace,
    },
    #[snafu(display("Rustix error: {source}"), visibility(pub(crate)))]
    RustixIo {
        source: rustix::io::Errno,
        #[snafu(backtrace)]
        backtrace: Backtrace,
    },
}
impl From<rustix::io::Errno> for ProtocolError {
    fn from(source: rustix::io::Errno) -> Self {
        ProtocolError::RustixIo {
            source,
            backtrace: Backtrace::capture(),
        }
    }
}

impl From<std::io::Error> for ProtocolError {
    fn from(source: std::io::Error) -> Self {
        ProtocolError::Io {
            source,
            backtrace: Backtrace::capture(),
        }
    }
}

impl From<serde_cbor::Error> for ProtocolError {
    fn from(source: serde_cbor::Error) -> Self {
        ProtocolError::Cbor {
            source,
            backtrace: Backtrace::capture(),
        }
    }
}

const SC_HDR_ALIGN: u32 =
    (usize::BITS - 1) - (align_of::<SnapshotHeader>()).leading_zeros();

// creates a shared memory region
// and copies the snapshot into it
// TODO better errors please
pub fn create_shmem_and_write_snapshot(
    state: &JobsStateInner,
    snap_seq_uid: u64,
    pid: i32,
) -> Result<SnapshotMemfd, ProtocolError> {
    let state_wire: JobsStateInnerWire = state.clone().into();

    // we need to calculate the size of the shit we're sending
    let state_blob = cbor::to_vec(&state_wire)?;

    let header_len = size_of::<SnapshotHeader>() as u64;
    let len_state_blob = state_blob.len() as u64;

    // this is just in case the page size is < 4 required for the struct (how??)
    let total_len_struct_aligned =
        align_up_pow2(header_len + len_state_blob, SC_HDR_ALIGN);
    let total_len_snapshot = round_up_page(total_len_struct_aligned);

    let name = format!("nix-btm-snapshot-p{pid}");
    let mut shmem = Shm::open(
        &name,
        // TODO I use these flags elsewhere. Save into global constant
        OFlags::CREATE | OFlags::EXCL | OFlags::RDWR,
        // TODO I use these magic numbers elsewhere. Save into global constant
        // with semantic meaning
        Mode::from_bits_truncate(0o600),
    )?;
    shmem.set_size(total_len_snapshot as usize)?;
    // TODO I use these elsewehre. Save into global constant
    let mut mappedmem = unsafe { shmem.map(0x0)? };
    let buf = mappedmem.map();

    let hdr = SnapshotHeader::new(len_state_blob, snap_seq_uid);
    let hdr_bytes = bytemuck::bytes_of(&hdr);
    buf[..hdr_bytes.len()].copy_from_slice(hdr_bytes);
    let start_state_blob = header_len as usize;
    let end_state_blob = start_state_blob + len_state_blob as usize;
    buf[start_state_blob..end_state_blob].copy_from_slice(&state_blob);

    buf[..core::mem::size_of::<SnapshotHeader>()]
        .copy_from_slice(bytes_of(&hdr));
    Ok(SnapshotMemfd {
        shmem: UnlinkOnDrop { shm: shmem },
        total_len_bytes: total_len_snapshot,
        snap_seq_uid,
    })
}
