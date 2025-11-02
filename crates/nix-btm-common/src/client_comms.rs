use std::ffi::c_void;

use bytemuck::from_bytes;
use rustix::mm::{MapFlags, ProtFlags, mmap, munmap};
use serde_cbor as cbor;

use crate::{
    client_daemon_comms::{JobsStateInnerWire, SnapshotHeader},
    handle_internal_json::JobsStateInner,
};

pub fn client_read_snapshot_into_state(
    fd: &rustix::fd::OwnedFd,
    total_len: u64,
) -> Option<JobsStateInner> {
    let ptr = unsafe {
        mmap(
            core::ptr::null_mut(),
            total_len as usize,
            ProtFlags::READ,
            MapFlags::SHARED,
            fd,
            0,
        )
        .ok()? as *const u8
    };
    let bytes = unsafe { core::slice::from_raw_parts(ptr, total_len as usize) };

    let hsz = core::mem::size_of::<SnapshotHeader>();
    let hdr: &SnapshotHeader = from_bytes(&bytes[..hsz]);

    // FAIL!
    if hdr.magic != SnapshotHeader::MAGIC
        || hdr.version != SnapshotHeader::VERSION
    {
        unsafe {
            let _ = munmap(bytes.as_ptr() as *mut c_void, bytes.len());
        }
        return None;
    }

    let state_blob = &bytes
        [hdr.header_len as usize..(hdr.header_len + hdr.payload_len) as usize];

    let state: JobsStateInner =
        cbor::from_slice::<JobsStateInnerWire>(state_blob)
            .ok()?
            .into();

    unsafe {
        let _ = munmap(bytes.as_ptr() as *mut c_void, bytes.len());
    }

    Some(state)
}
