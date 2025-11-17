use std::{
    backtrace::Backtrace,
    mem::size_of,
    os::fd::{AsRawFd, BorrowedFd, RawFd},
    ptr,
    sync::atomic::Ordering,
};

use bytemuck::try_from_bytes;
use io_uring::{IoUring, Probe, opcode::FutexWait};
use memmap2::{Mmap, MmapOptions};
use rustix::{
    fs::fstat,
    mm::{MapFlags, ProtFlags, mmap},
};
use snafu::{GenerateImplicitData, ResultExt as _, ensure};

use crate::{
    daemon_side::align_up_pow2,
    protocol_common::{
        IoSnafu, IoUringNotSupportedSnafu, Kind, MisMatchSnafu, ProtocolError,
        RustixIoSnafu, ShmHeader, ShmHeaderView, ShmRecordHeader, Update,
    },
    ring_writer::{MAX_NUM_CLIENTS, RING_ALIGN_SHIFT, SHM_RECORD_HDR_SIZE},
};

pub enum ReadResult {
    Update { seq: u32, update: Update },
    Lost { from: u32, to: u32 },
    NoUpdate,
    NeedCatchup,
}

pub struct RingReader {
    map: Mmap,
    hdr_ptr: *mut ShmHeader,
    ring_len: u32,
    off: u32,
    next_seq: u32,
    uring: IoUring,
    fd: RawFd,
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
        //let len = st.st_size;
        //ensure!( todo!()
        //    len as usize == expected_shm_len,
        //    UnexpectedRingSizeSnafu {
        //        expected: expected_shm_len as u64,
        //        got: len as u64
        //    }
        //);

        let mmaped_region = unsafe {
            MmapOptions::new()
                .len(expected_shm_len)
                .map(fd.as_raw_fd())?
        };

        let hdr_ptr = mmaped_region.as_ptr() as *const ShmHeader;
        let (ring_len, off, next_seq) = {
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

            // Initialize reader to start from the beginning of valid data
            let (start_seq, start_offset) = hv.read_start_seq_and_offset();
            let (end_seq, _end_offset) = hv.read_seq_and_next_entry_offset();

            // If start_seq is 0 (initial state) but there's data (end_seq > 0),
            // the first record will have seq=1, so start from seq 1
            let next_seq = if start_seq == 0 && end_seq > 0 {
                1
            } else {
                start_seq
            };

            (ring_len, start_offset, next_seq)
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
            fd,
            uring,
        })
    }

    /// Get the ring buffer data slice (after the header)
    #[inline]
    fn ring_slice_as_bytes(&self) -> &[u8] {
        let hdr_size = size_of::<ShmHeader>();
        &self.map[hdr_size..hdr_size + self.ring_len as usize]
    }

    /// Check if the reader's current position is still valid, then try to read.
    /// Returns NeedCatchup if the reader has fallen too far behind or data is
    /// invalid.
    pub fn try_read(&mut self) -> ReadResult {
        std::sync::atomic::fence(Ordering::Acquire);
        let hv = ShmHeaderView::new(self.hdr_ptr);
        let (start_seq, start_offset) = hv.read_start_seq_and_offset();
        let (_end_seq, end_offset) = hv.read_seq_and_next_entry_offset();

        // we're out of sync
        if self.next_seq < start_seq {
            return ReadResult::NeedCatchup;
        }

        let offset_valid = if end_offset > start_offset {
            // no wraparound
            self.off >= start_offset && self.off < end_offset
        } else if end_offset < start_offset {
            // Wraparound case: valid range is [start_offset, ring_len) U [0,
            // end_offset)
            self.off >= start_offset || self.off < end_offset
        } else {
            // end_offset == start_offset: ring is empty
            return ReadResult::NoUpdate;
        };

        if !offset_valid {
            return ReadResult::NeedCatchup;
        }

        // try to read the record immediately before it can be
        // overwritten
        match self.try_parse_current_record() {
            Ok(Some(update)) => update,
            Ok(None) => ReadResult::NoUpdate,
            Err(_) => ReadResult::NeedCatchup,
        }
    }

    fn try_parse_current_record(
        &mut self,
    ) -> Result<Option<ReadResult>, ProtocolError> {
        let ring_bytes = self.ring_slice_as_bytes();

        // Read record header at current offset
        let hdr_start = self.off as usize;
        let hdr_end = hdr_start + SHM_RECORD_HDR_SIZE as usize;
        let hdr_bytes = &ring_bytes[hdr_start..hdr_end];

        let rec_hdr: &ShmRecordHeader =
            try_from_bytes(hdr_bytes).map_err(|_| {
                ProtocolError::MisMatchError {
                    backtrace: snafu::Backtrace::generate(),
                }
            })?;

        // if padding wrap around
        if rec_hdr.payload_kind == (Kind::Padding as u32) {
            self.off = 0;
            self.next_seq = self.next_seq.wrapping_add(1);
            return Ok(None);
        }

        // if seq matches
        if rec_hdr.seq != self.next_seq {
            // Sequence mismatch
            if rec_hdr.seq > self.next_seq {
                let from = self.next_seq;
                let to = rec_hdr.seq;
                self.next_seq = rec_hdr.seq.wrapping_add(1);
                return Ok(Some(ReadResult::Lost { from, to }));
            } else {
                // seq < next_seq shouldn't happen, data is stale/corrupted
                return Err(ProtocolError::MisMatchError {
                    backtrace: snafu::Backtrace::generate(),
                });
            }
        }

        let payload_start = self.off as usize + SHM_RECORD_HDR_SIZE as usize;
        let payload_end = payload_start + rec_hdr.payload_len as usize;
        let payload_bytes = &ring_bytes[payload_start..payload_end];

        let update: Update = serde_cbor::from_slice(payload_bytes)
            .context(crate::protocol_common::CborSnafu)?;

        let rec_size = align_up_pow2(
            SHM_RECORD_HDR_SIZE + rec_hdr.payload_len,
            RING_ALIGN_SHIFT,
        );
        self.off = (self.off + rec_size) % self.ring_len;
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);

        Ok(Some(ReadResult::Update { seq, update }))
    }
}
