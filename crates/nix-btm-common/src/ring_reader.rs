use std::{
    backtrace::Backtrace,
    os::fd::{AsRawFd, BorrowedFd, RawFd},
    sync::atomic::Ordering,
};

use io_uring::{IoUring, Probe, opcode::FutexWait};
use memmap2::{Mmap, MmapMut, MmapOptions};
use psx_shm::Shm;
use rustix::fs::fstat;
use snafu::{GenerateImplicitData, ResultExt as _, ensure};

use crate::{
    daemon_side::{IoSnafu, MisMatchSnafu, ProtocolError, RustixIoSnafu},
    protocol_common::{ShmHeader, ShmHeaderView, Update},
};

pub enum ReadResult {
    Update { seq: u32, update: Update },
    Lost { from: u32, to: u32 },
    NoUpdate,
}

pub struct RingReader {
    map: Mmap,
    hdr_ptr: *mut ShmHeader,
    ring_len: usize,
    off: usize,
    next_seq: u32,
    uring: IoUring,
}

impl RingReader {
    pub fn from_fd(fd: RawFd) -> Result<Self, ProtocolError> {
        // Borrow the fd and get size
        let bf = unsafe { BorrowedFd::borrow_raw(fd) };
        let st = fstat(&bf).context(RustixIoSnafu)?;
        let len = st.st_size as usize;
        ensure!(len >= size_of::<ShmHeader>(), MisMatchSnafu);

        // Map read-only (clients never write)
        let map = unsafe { MmapOptions::new().len(len).map(bf.as_raw_fd()) }
            .map_err(|source| ProtocolError::Io {
                source,
                backtrace: Backtrace::generate(),
            })?;

        // Validate header & initialize cursors
        let hdr_ptr = map.as_ptr() as *mut ShmHeader;
        let (ring_len, off, next_seq) = unsafe {
            std::sync::atomic::fence(Ordering::Acquire);
            let hv = ShmHeaderView::new(&mut *hdr_ptr);
            ensure!(
                hv.magic() == ShmHeader::MAGIC
                    && hv.version() == ShmHeader::VERSION,
                MisMatchSnafu
            );
            let ring_len = hv.ring_len();
            let off =
                hv.write_next_entry_offset().load(Ordering::Acquire) as usize;
            let next_seq =
                hv.write_seq().load(Ordering::Acquire).wrapping_add(1);
            (ring_len, off, next_seq)
        };

        // io_uring with FutexWait support
        let uring = IoUring::new(256).context(IoSnafu)?;
        let mut probe = Probe::new();
        let _ = uring.submitter().register_probe(&mut probe);
        ensure!(probe.is_supported(FutexWait::CODE), MisMatchSnafu);

        Ok(Self {
            map,
            hdr_ptr,
            ring_len: ring_len as usize,
            off,
            next_seq,
            uring,
        })
    }
}
