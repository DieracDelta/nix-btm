use std::{
    mem::size_of,
    os::fd::{AsRawFd, RawFd},
    sync::atomic::Ordering,
};

use bytemuck::try_from_bytes;
use io_uring::{IoUring, Probe, opcode::FutexWait};
use memmap2::Mmap;
use snafu::{GenerateImplicitData, ResultExt};

use crate::{
    daemon_side::align_up_pow2,
    protocol_common::{
        Kind, ProtocolError,
        ShmHeader, ShmHeaderView, ShmRecordHeader, Update,
    },
    ring_writer::{MAX_NUM_CLIENTS, RING_ALIGN_SHIFT, SHM_RECORD_HDR_SIZE},
};

#[derive(Debug)]
pub enum ReadResult {
    Update { seq: u32, update: Update },
    Lost { from: u32, to: u32 },
    NoUpdate,
    NeedCatchup,
}

pub struct RingReader {
    map: Mmap,
    hdr_ptr: *mut ShmHeader,
    pub ring_len: u32,
    off: u32,
    next_seq: u32,
    uring: Option<IoUring>, // None if io_uring not supported
    fd: RawFd,
}

impl RingReader {
    pub fn from_name(
        name: &str,
        expected_shm_len: usize,
    ) -> Result<Self, ProtocolError> {
        use psx_shm::Shm;
        use rustix::shm::OFlags;
        use rustix::fs::Mode;

        // Open existing shared memory
        let shm = Shm::open(
            name,
            OFlags::RDONLY,
            Mode::from_bits_truncate(0o600),
        )?;

        // Map using memmap2 directly on the file descriptor
        let fd = shm.as_fd();
        let mmaped_region: Mmap = unsafe {
            Mmap::map(fd.as_raw_fd())?
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

        // Try to initialize io_uring with FutexWait support
        // If not available, fall back to POSIX-only mode (polling)
        // Can be disabled with DISABLE_IO_URING=1 environment variable for testing
        let uring = if std::env::var("DISABLE_IO_URING").is_ok() {
            eprintln!("Info: io_uring disabled via DISABLE_IO_URING, using POSIX mode (polling only)");
            None
        } else {
            match IoUring::new(MAX_NUM_CLIENTS) {
                Ok(uring) => {
                    let mut probe = Probe::new();
                    let _ = uring.submitter().register_probe(&mut probe);
                    if probe.is_supported(FutexWait::CODE) {
                        Some(uring)
                    } else {
                        eprintln!("Warning: io_uring FutexWait not supported, falling back to POSIX mode (polling only)");
                        None
                    }
                }
                Err(_) => {
                    eprintln!("Warning: io_uring not available, falling back to POSIX mode (polling only)");
                    None
                }
            }
        };

        let fd = fd.as_raw_fd();

        Ok(Self {
            map: mmaped_region,
            hdr_ptr: (hdr_ptr as *mut ShmHeader),
            ring_len,
            off,
            next_seq,
            uring,
            fd,
        })
    }

    /// Get the ring buffer data slice (after the header)
    #[inline]
    fn ring_slice_as_bytes(&self) -> &[u8] {
        let hdr_size = size_of::<ShmHeader>();
        &self.map[hdr_size..hdr_size + self.ring_len as usize]
    }

    /// Try to read data immediately, then validate the read was safe.
    /// Returns NeedCatchup if the reader has fallen too far behind or data is
    /// invalid.
    pub fn try_read(&mut self) -> ReadResult {
        std::sync::atomic::fence(Ordering::Acquire);

        // Step 1: Save current position and read immediately (optimistic read)
        let original_offset = self.off;
        let original_next_seq = self.next_seq;
        let parse_result = self.try_parse_current_record();

        // Step 2: Check atomics to verify the read was valid
        let hv = ShmHeaderView::new(self.hdr_ptr);
        let (start_seq, start_offset) = hv.read_start_seq_and_offset();
        let (_end_seq, end_offset) = hv.read_seq_and_next_entry_offset();

        // Check 1: Are we out of sync (lapped)?
        if original_next_seq < start_seq {
            return ReadResult::NeedCatchup;
        }

        // Check 2: Was the original offset in valid range?
        let was_offset_valid = if end_offset > start_offset {
            // no wraparound
            original_offset >= start_offset && original_offset < end_offset
        } else if end_offset < start_offset {
            // Wraparound case: valid range is [start_offset, ring_len) U [0,
            // end_offset)
            original_offset >= start_offset || original_offset < end_offset
        } else {
            // end_offset == start_offset: ring is empty
            return ReadResult::NoUpdate;
        };

        if !was_offset_valid {
            return ReadResult::NeedCatchup;
        }

        // Step 3: Data was valid, return the parse result
        match parse_result {
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
