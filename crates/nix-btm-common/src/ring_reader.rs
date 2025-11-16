use std::{
    backtrace::Backtrace,
    os::fd::{AsRawFd, BorrowedFd, RawFd},
    ptr,
    sync::atomic::Ordering,
};

use io_uring::{IoUring, Probe, opcode::FutexWait};
use memmap2::{Mmap, MmapOptions};
use rustix::{
    fs::fstat,
    mm::{MapFlags, ProtFlags, mmap},
};
use snafu::{GenerateImplicitData, ResultExt as _, ensure};

use crate::{
    protocol_common::{
        IoSnafu, IoUringNotSupportedSnafu, ProtocolError, RustixIoSnafu,
        ShmHeader, ShmHeaderView, Update,
    },
    ring_writer::MAX_NUM_CLIENTS,
};

pub enum ReadResult {
    Update { seq: u32, update: Update },
    Lost { from: u32, to: u32 },
    NoUpdate,
}

pub struct RingReader {
    map: Mmap,
    hdr_ptr: *mut ShmHeader,
    ring_len: u32,
    off: u32,
    next_seq: u32,
    uring: IoUring,
}

impl RingReader {
    pub fn from_fd(
        fd: RawFd,
        expected_shm_len: usize,
    ) -> Result<Self, ProtocolError> {
        let bf = unsafe { BorrowedFd::borrow_raw(fd) };
        let st = fstat(bf).context(RustixIoSnafu)?;

        // sanity check size
        // TODO in prod probably want to hide this behind a sanity check feature
        // flag
        let len = st.st_size;
        //ensure!( todo!()
        //    len as usize == expected_shm_len,
        //    UnexpectedRingSizeSnafu {
        //        expected: expected_shm_len as u64,
        //        got: len as u64
        //    }
        //);

        let mmaped_region = unsafe {
            MmapOptions::new()
                .len(expected_shm_len as usize)
                .map(fd.as_raw_fd())?
        };

        let hdr_ptr = mmaped_region.as_ptr() as *const ShmHeader;
        let (ring_len, off, next_seq) = unsafe {
            std::sync::atomic::fence(Ordering::Acquire);
            let hv = ShmHeaderView::new(hdr_ptr);
            //ensure!(
            //    hv.magic() == ShmHeader::MAGIC,
            //    UnexpectedRingSizeSnafu {
            //        expected: ShmHeader::MAGIC,
            //        got: hv.magic(),
            //    }
            //);
            //ensure!(
            //    hv.version() == ShmHeader::VERSION,
            //    UnexpectedRingSizeSnafu {
            //        expected: ShmHeader::VERSION,
            //        got: hv.version(),
            //    }
            //);
            // safe to do this read
            let ring_len = hv.ring_len();

            // atomics for the reset
            let (cur_seq, off) = hv.read_seq_and_next_entry_offset();
            let next_seq = cur_seq.wrapping_add(1);
            (ring_len, off, next_seq)
        };

        // io_uring with FutexWait support
        let uring = IoUring::new(MAX_NUM_CLIENTS).context(IoSnafu)?;
        let mut probe = Probe::new();
        let _ = uring.submitter().register_probe(&mut probe);
        ensure!(
            probe.is_supported(FutexWait::CODE),
            IoUringNotSupportedSnafu
        );

        Ok(Self {
            map: mmaped_region,
            hdr_ptr: (hdr_ptr as *mut ShmHeader),
            ring_len,
            off,
            next_seq,
            uring,
        })
    }
}
