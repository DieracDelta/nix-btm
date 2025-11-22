use std::os::fd::OwnedFd;

use bytemuck::from_bytes;
use memmap2::MmapOptions;
use psx_shm::Shm;
use rustix::{io::dup, shm::OFlags};
use serde_cbor as cbor;
use snafu::GenerateImplicitData;

use crate::{
    handle_internal_json::JobsStateInner,
    protocol_common::{
        JobsStateInnerWire, ProtocolError, R_MODE, SnapshotHeader,
    },
};

pub fn client_read_snapshot_into_state(
    name: &str,
    total_len: u64,
) -> Result<JobsStateInner, ProtocolError> {
    let shmem = Shm::open(name, OFlags::RDONLY, R_MODE)?;
    let dup_fd: OwnedFd = dup(shmem.as_fd())?;
    let file = std::fs::File::from(dup_fd);

    let sz = shmem.size()?;
    if sz < total_len as usize {
        return Err(ProtocolError::MisMatchError {
            backtrace: snafu::Backtrace::generate(),
        });
    }
    // TODO make Mmap able to get out and make PR
    let map = unsafe { MmapOptions::new().len(sz).map(&file)? };
    let bytes: &[u8] = &map;

    let hsz = core::mem::size_of::<SnapshotHeader>();
    let hdr: &SnapshotHeader = from_bytes(&bytes[..hsz]);

    // TODO fix
    // FAIL!
    //if hdr.magic != SnapshotHeader::MAGIC
    //    || hdr.version != SnapshotHeader::VERSION
    //{
    //    return Err(ProtocolError::MisMatchError {
    //        backtrace: Backtrace::capture(),
    //    });
    //}

    let state_blob = &bytes
        [hdr.header_len as usize..(hdr.header_len + hdr.payload_len) as usize];

    let state: JobsStateInner =
        cbor::from_slice::<JobsStateInnerWire>(state_blob)?.into();

    Ok(state)
}
