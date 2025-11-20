use std::sync::atomic::Ordering;

use bytemuck::{Pod, bytes_of};
use psx_shm::Shm;
use rustix::{
    fs::Mode,
    shm::OFlags,
};
use snafu::ResultExt;

// TODO should be able to specify this from cli
pub const MAX_NUM_CLIENTS: u32 = 256;

use crate::{
    daemon_side::align_up_pow2,
    notify::Notifier,
    protocol_common::{
        CborSnafu, Kind, ProtocolError, ShmHeaderView,
        ShmRecordHeader, Update,
    },
};

/// how much to shift to get alignment
const SHM_HDR_SIZE: u32 = size_of::<ShmHeader>() as u32;
#[allow(dead_code)]
const SHM_HDR_SIZE_ALIGNED: u32 = align_up_pow2(SHM_HDR_SIZE, RING_ALIGN_SHIFT);

pub(crate) const SHM_RECORD_HDR_SIZE: u32 = size_of::<ShmRecordHeader>() as u32;
//const SHM_RECORD_HDR_SIZE_ALIGNED: u32 =
//    align_up_pow2(SHM_RECORD_HDR_SIZE, RING_ALIGN_SHIFT);
pub(crate) const RING_ALIGN_SHIFT: u32 = 3;

use crate::protocol_common::ShmHeader;

// TODO some pieces of this may change depending on usage
pub struct RingWriter {
    notifier: Option<Notifier>, // Platform-specific notifier (io_uring on Linux, kqueue on macOS)
    _shm: Shm, // Keep shm alive but don't use it directly
    base_ptr: *mut u8, // Base pointer to mapped memory
    hdr: *mut ShmHeader,
    pub ring_len: u32,
    pub name: String,
}

unsafe impl Send for RingWriter {}
unsafe impl Sync for RingWriter {}

impl RingWriter {
    pub fn create(name: &str, ring_len: u32) -> Result<Self, ProtocolError> {
        let total_len = SHM_HDR_SIZE + ring_len;

        // Create named shared memory
        let mut shm = Shm::open(
            name,
            OFlags::CREATE | OFlags::EXCL | OFlags::RDWR,
            Mode::from_bits_truncate(0o600),
        )?;
        shm.set_size(total_len as usize)?;

        let mut mapped = unsafe { shm.map(0x0)? };
        let base = mapped.map().as_mut_ptr();
        let hdr = base as *mut ShmHeader;

        unsafe {
            (*hdr).magic = ShmHeader::MAGIC;
            (*hdr).version = ShmHeader::VERSION;
            (*hdr).ring_len = ring_len;
        }

        let hv = ShmHeaderView::new(hdr);
        hv.write_seq_and_next_entry_offset(0, 0);
        hv.write_start_seq_and_offset(0, 0);

        // Initialize platform-specific notifier (io_uring on Linux, kqueue on macOS)
        let notifier = Notifier::new()?;

        // SAFETY: We keep the shm alive and don't unlink it, so the mapping remains valid
        // We leak the BorrowedMap to get a 'static lifetime since we manage the Shm lifetime ourselves
        let base_ptr = base;
        std::mem::forget(mapped);

        Ok(Self {
            notifier,
            _shm: shm,
            base_ptr,
            hdr,
            ring_len,
            name: name.to_string(),
        })
    }

    #[inline]
    fn wake_readers(&mut self) -> Result<(), ProtocolError> {
        // Use platform-specific notifier to wake readers
        // Otherwise, readers will poll (POSIX fallback mode)
        if self.notifier.is_some() {
            let hv = self.header_view();
            let uaddr: *const u32 = hv.get_seq_ptr();
            self.notifier.as_mut().unwrap().wake(uaddr)?;
        }
        // In POSIX mode, do nothing - readers will poll

        Ok(())
    }

    #[inline]
    pub fn header_view(&self) -> ShmHeaderView<'_> {
        unsafe { ShmHeaderView::new(&*self.hdr) }
    }

    fn ring_mut(&mut self) -> &mut [u8] {
        let hdr_sz = SHM_HDR_SIZE;
        unsafe {
            std::slice::from_raw_parts_mut(
                self.base_ptr.add(hdr_sz as usize),
                self.ring_len as usize
            )
        }
    }

    pub fn write_update(&mut self, upd: &Update) -> Result<u32, ProtocolError> {
        let kind_num = match upd {
            Update::JobNew(_) => Kind::JobNew as u32,
            Update::JobUpdate { .. } => Kind::JobUpdate as u32,
            Update::JobFinish { .. } => Kind::JobFinish as u32,
            Update::DepGraphUpdate { .. } => Kind::DepGraphUpdate as u32,
            Update::Heartbeat { .. } => Kind::Heartbeat as u32,
        };

        let bytes = serde_cbor::to_vec(upd).context(CborSnafu)?;
        self.write_update_raw(kind_num, &bytes)
    }

    #[inline]
    pub fn put_pod_at<T: Pod>(
        &mut self,
        ring_off: u64,
        v: &T,
    ) -> Result<(), ProtocolError> {
        self.put_bytes_at(ring_off, bytes_of(v))
    }

    #[inline]
    pub fn put_bytes_at(
        &mut self,
        ring_off: u64,
        v: &[u8],
    ) -> Result<(), ProtocolError> {
        let ring = self.ring_mut();
        // TODO fix
        let pod_end = (ring_off as usize).checked_add(v.len()).unwrap();
        //.ok_or_else(|| panic!("oh no"));
        //ensure!(pod_end <= ring.len(), MisMatchSnafu);
        ring[ring_off as usize..pod_end].copy_from_slice(v);
        Ok(())
    }

    pub fn write_update_raw(
        &mut self,
        kind: u32,
        payload: &[u8],
    ) -> Result<u32, ProtocolError> {
        let ring_len = self.ring_len;
        let space_required_for_payload = align_up_pow2(
            SHM_RECORD_HDR_SIZE + (payload.len() as u32),
            RING_ALIGN_SHIFT,
        );
        // this really shouldn't be possible
        // TODO re-enable
        //ensure!(space_required_for_payload <= ring_len, MisMatchSnafu);

        // calculate metadata for writing first
        let (seq, mut offset_to_new_update) = {
            let hv = self.header_view();
            let (prev_seq, prev_offset) = hv.read_seq_and_next_entry_offset();
            let seq = prev_seq.wrapping_add(1);
            (seq, prev_offset)
        };

        let remain: u32 = (ring_len as u32) - offset_to_new_update;

        // if we can't fit the entire thing in, but we can fit the header, then
        // just fit the header
        if space_required_for_payload as u32 > remain {
            if remain >= SHM_RECORD_HDR_SIZE {
                let pad_hdr = ShmRecordHeader {
                    payload_kind: Kind::Padding.into(),
                    payload_len: 0,
                    seq,
                };
                self.put_pod_at(offset_to_new_update as u64, &pad_hdr)?;
            }
            std::sync::atomic::fence(Ordering::Release);

            // Wraparound: update start_offset to invalidate old data
            let hv = self.header_view();
            let (prev_start_seq, _prev_start_offset) =
                hv.read_start_seq_and_offset();
            let new_start_seq = prev_start_seq.wrapping_add(1);
            hv.write_start_seq_and_offset(new_start_seq, 0);

            offset_to_new_update = 0;
            // TODO in this path don't I need to increment the seq number again?
        }

        let header = ShmRecordHeader {
            payload_kind: kind,
            payload_len: payload.len() as u32,
            seq,
        };

        // write payload THEN header
        self.put_bytes_at(
            SHM_RECORD_HDR_SIZE as u64 + offset_to_new_update as u64,
            payload,
        )?;
        // write header
        self.put_pod_at(offset_to_new_update as u64, &header)?;
        std::sync::atomic::fence(Ordering::Release);

        // finally update header
        {
            let hv = self.header_view();
            let next_entry_offset: u32 =
                (offset_to_new_update + space_required_for_payload) % ring_len;
            hv.write_seq_and_next_entry_offset(seq, next_entry_offset);
        }

        self.wake_readers()?;

        Ok(seq)
    }
}
