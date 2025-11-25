use std::{mem::size_of, os::fd::AsRawFd, sync::atomic::Ordering};

use bytemuck::try_from_bytes;
use memmap2::Mmap;
use snafu::{GenerateImplicitData, ResultExt};

use crate::{
    daemon_side::align_up_pow2,
    notify::Waiter,
    protocol_common::{
        Kind, ProtocolError, ShmHeader, ShmHeaderView, ShmRecordHeader, Update,
    },
    ring_writer::{RING_ALIGN_SHIFT, SHM_RECORD_HDR_SIZE},
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
    #[allow(dead_code)]
    waiter: Option<Waiter>, /* Platform-specific waiter (io_uring on Linux,
                             * kqueue on macOS) */
}

// SAFETY: The raw pointer points to memory-mapped shared memory that remains
// valid for the lifetime of RingReader. The Mmap keeps the mapping alive.
unsafe impl Send for RingReader {}

impl RingReader {
    pub fn from_name(
        name: &str,
        _expected_shm_len: usize,
    ) -> Result<Self, ProtocolError> {
        use psx_shm::Shm;
        use rustix::{fs::Mode, shm::OFlags};

        // Open existing shared memory
        let shm =
            Shm::open(name, OFlags::RDONLY, Mode::from_bits_truncate(0o600))?;

        // Map using memmap2 directly on the file descriptor
        let fd = shm.as_fd();
        let mmaped_region: Mmap = unsafe { Mmap::map(fd.as_raw_fd())? };

        let hdr_ptr = mmaped_region.as_ptr() as *const ShmHeader;
        let ring_len = {
            std::sync::atomic::fence(Ordering::Acquire);
            let hv = ShmHeaderView::new(hdr_ptr);
            hv.ring_len()
        };

        // Initialize platform-specific waiter (io_uring on Linux, kqueue on
        // macOS)
        let waiter = Waiter::new()?;

        // Initialize reader to current buffer position
        // This allows immediate reading if buffer has data, or waiting for new
        // data
        let hv = ShmHeaderView::new(hdr_ptr);
        let (end_seq, end_offset) = hv.read_seq_and_next_entry_offset();

        // If buffer is empty (end_seq=0), wait for seq 1
        // If buffer has data, start from current end to wait for new data
        let (init_seq, init_off) = if end_seq == 0 {
            // Empty buffer - wait for first write which will be seq 1
            (1, 0)
        } else {
            // Buffer has data - position at end to wait for new data
            // Caller should use sync_to_snapshot for catchup scenarios
            (end_seq.wrapping_add(1), end_offset)
        };

        Ok(Self {
            map: mmaped_region,
            hdr_ptr: (hdr_ptr as *mut ShmHeader),
            ring_len,
            off: init_off,
            next_seq: init_seq,
            waiter,
        })
    }

    /// Get the ring buffer data slice (after the header)
    #[inline]
    fn ring_slice_as_bytes(&self) -> &[u8] {
        let hdr_size = size_of::<ShmHeader>();
        &self.map[hdr_size..hdr_size + self.ring_len as usize]
    }

    /// Sync reader to the current end of the ring buffer.
    /// This should be called after loading a snapshot so the client waits for
    /// new data. The snapshot already contains all state up to snap_seq, so
    /// we start from the current end.
    pub fn sync_to_snapshot(&mut self, _snap_seq: u64) {
        std::sync::atomic::fence(Ordering::Acquire);
        let hv = ShmHeaderView::new(self.hdr_ptr);
        let (end_seq, end_offset) = hv.read_seq_and_next_entry_offset();
        let (start_seq, start_offset) = hv.read_start_seq_and_offset();

        tracing::error!(
            "sync_to_snapshot: start=({}, {}), end=({}, {})",
            start_seq,
            start_offset,
            end_seq,
            end_offset
        );

        // Position at the current end of the buffer to wait for new data
        // The snapshot already contains all historical state
        // end_seq is the last written sequence, so we wait for end_seq + 1
        self.next_seq = end_seq.wrapping_add(1);
        self.off = end_offset;
    }

    /// Try to read the next update from the ring buffer.
    /// Returns NeedCatchup if the reader has fallen too far behind.
    pub fn try_read(&mut self) -> ReadResult {
        std::sync::atomic::fence(Ordering::Acquire);

        let hv = ShmHeaderView::new(self.hdr_ptr);
        let (start_seq, _start_offset) = hv.read_start_seq_and_offset();
        let (end_seq, _end_offset) = hv.read_seq_and_next_entry_offset();

        // Check if we're out of sync (lapped by writer)
        if self.next_seq > 0 && self.next_seq < start_seq {
            return ReadResult::NeedCatchup;
        }

        // Check if we're at the end waiting for new data
        // end_seq is the last written sequence, so we can read up to and
        // including it
        if self.next_seq > end_seq {
            return ReadResult::NoUpdate;
        }

        // Try to parse the next record
        match self.try_parse_current_record() {
            Ok(Some(update)) => update,
            Ok(None) => ReadResult::NoUpdate,
            Err(_) => ReadResult::NeedCatchup,
        }
    }

    /// Wait for new data using platform-specific notification (futex/kqueue).
    /// Falls back to returning immediately if no waiter is available.
    /// Returns the current end sequence number after waking.
    pub fn wait_for_update(
        &mut self,
    ) -> Result<(), crate::protocol_common::ProtocolError> {
        if let Some(ref mut waiter) = self.waiter {
            // Get the address of the end_seq field in shared memory header
            // The ShmHeader has seq_and_offset at offset 0 (u64)
            // We want to wait on the sequence part (low 32 bits)
            let seq_addr = self.hdr_ptr as *const u32;

            // Get current end sequence
            let hv = ShmHeaderView::new(self.hdr_ptr);
            let (end_seq, _) = hv.read_seq_and_next_entry_offset();

            // Only wait if we're caught up
            if self.next_seq > end_seq {
                waiter.wait(seq_addr, end_seq)?;
            }
        }
        Ok(())
    }

    /// Check if this reader has a waiter available for efficient blocking.
    pub fn has_waiter(&self) -> bool {
        self.waiter.is_some()
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
                // Update next_seq to current record's seq so next read will
                // succeed Don't update offset - we'll read this
                // record on next call
                self.next_seq = rec_hdr.seq;
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
