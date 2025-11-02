use std::{
    ffi::CString,
    fmt::Debug,
    os::fd::{AsFd, OwnedFd},
};

use bytemuck::bytes_of;
pub use rustix::*;
use rustix::{
    fs::{MemfdFlags, ftruncate, memfd_create},
    mm::{MapFlags, ProtFlags, mmap},
};
use serde_cbor as cbor;
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
fn round_up_page(num_bytes: u64) -> u64 {
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
    pub fd: OwnedFd,
    pub total_len_bytes: u64,
    pub snap_seq_uid: u64,
}

// creates a shared memory region
// and copies the snapshot into it
pub fn create_shmem_and_write_snapshot(
    state: &JobsStateInner,
    snap_seq_uid: u64,
    pid: i32,
) -> Option<SnapshotMemfd> {
    let state_wire: JobsStateInnerWire = state.clone().into();

    // we need to calculate the size of the shit we're sending
    let state_blob = cbor::to_vec(&state_wire).ok()?;

    // we are rounding up each time which might be extra but who cares
    let header_len = size_of::<SnapshotHeader>() as u64;
    let mut off = align_up_pow2(header_len, 4);

    let off_jobs = off;
    let len_state_blob = state_blob.len() as u64;
    off = align_up_pow2(off_jobs + len_state_blob, 4);
    let total_len_snapshot = round_up_page(off);

    let name = CString::new(format!("nix-btm-snapshot-p{pid}")).ok()?;
    let fd: OwnedFd = memfd_create(&name, MemfdFlags::CLOEXEC).ok()?;
    ftruncate(&fd, total_len_snapshot).ok()?;
    let base = unsafe {
        mmap(
            core::ptr::null_mut(),
            total_len_snapshot as usize,
            ProtFlags::READ | ProtFlags::WRITE,
            MapFlags::SHARED,
            &fd,
            0,
        )
        .ok()? as *mut u8
    };
    let buf = unsafe {
        core::slice::from_raw_parts_mut(base, total_len_snapshot as usize)
    };

    let hdr = SnapshotHeader::new(len_state_blob, snap_seq_uid);
    let hdr_bytes = bytemuck::bytes_of(&hdr);
    buf[..hdr_bytes.len()].copy_from_slice(hdr_bytes);
    let start_state_blob = header_len as usize;
    let end_state_blob = start_state_blob + len_state_blob as usize;
    buf[start_state_blob..end_state_blob].copy_from_slice(&state_blob);

    buf[..core::mem::size_of::<SnapshotHeader>()]
        .copy_from_slice(bytes_of(&hdr));
    Some(SnapshotMemfd {
        fd,
        total_len_bytes: total_len_snapshot,
        snap_seq_uid,
    })
}
