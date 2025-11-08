use std::sync::atomic::Ordering;

use bytemuck::{Pod, bytes_of};
use memmap2::MmapMut;
use psx_shm::Shm;
use rustix::shm::OFlags;
use snafu::{ResultExt, ensure};

use crate::{
    daemon_side::{
        CborSnafu, IoSnafu, MisMatchSnafu, ProtocolError, align_up_pow2,
    },
    protocol_common::{Kind, ShmHeaderViewMut, ShmRecordHeader, Update},
};

/// how much to shift to get alignment
const SHM_HDR_SIZE: u64 = size_of::<ShmHeader>() as u64;
const SHM_HDR_SIZE_ALIGNED: u64 = align_up_pow2(SHM_HDR_SIZE, RING_ALIGN_SHIFT);

const SHM_RECORD_HDR_SIZE: u64 = size_of::<ShmRecordHeader>() as u64;
const SHM_RECORD_HDR_SIZE_ALIGNED: u64 =
    align_up_pow2(SHM_RECORD_HDR_SIZE, RING_ALIGN_SHIFT);
const RING_ALIGN_SHIFT: u32 = 3;

use crate::protocol_common::{RW_MODE, ShmHeader};

pub struct RingWriter {
    shm: Shm,
    map: MmapMut, // single mapping
    hdr: *mut ShmHeader,
    ring: *mut u8,
    ring_len: u64,
}

unsafe impl Send for RingWriter {}
unsafe impl Sync for RingWriter {}

impl RingWriter {
    pub fn create(name: &str, ring_len: u64) -> Result<Self, ProtocolError> {
        ensure!(
            ring_len.is_multiple_of(RING_ALIGN_SHIFT as u64),
            crate::daemon_side::MisMatchSnafu
        );

        let total_len = size_of::<ShmHeader>() as u64 + ring_len;

        let mut shm = Shm::open(
            name,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL,
            RW_MODE,
        )
        .context(IoSnafu)?;
        shm.set_size(total_len as usize).context(IoSnafu)?;

        let borrowed = unsafe { shm.map(0).context(IoSnafu)? };
        let map = unsafe { borrowed.into_map() };

        let base = map.as_ptr() as *mut u8;
        let hdr = base as *mut ShmHeader;
        let ring = unsafe { base.add(size_of::<ShmHeader>()) };

        unsafe {
            *hdr = ShmHeader {
                magic: ShmHeader::MAGIC,
                version: ShmHeader::VERSION,
                write_seq: 0,
                next_entry_offset: 0,
                ring_len: ring_len as u32,
            };
        }

        Ok(Self {
            shm,
            map,
            hdr,
            ring,
            ring_len,
        })
    }

    #[inline]
    fn header_view(&mut self) -> ShmHeaderViewMut<'_> {
        unsafe { ShmHeaderViewMut::new(&mut *self.hdr) }
    }

    fn ring_mut(&mut self) -> &mut [u8] {
        let hdr_sz = SHM_RECORD_HDR_SIZE;
        &mut self.map[hdr_sz as usize..hdr_sz as usize + self.ring_len as usize]
    }

    pub fn write_update(&mut self, upd: &Update) -> Result<u64, ProtocolError> {
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
        let pod_end = (ring_off as usize)
            .checked_add(v.len())
            .ok_or_else(|| MisMatchSnafu.build())?;
        ensure!(pod_end <= ring.len(), MisMatchSnafu);
        ring[ring_off as usize..pod_end].copy_from_slice(v);
        Ok(())
    }

    pub fn write_update_raw(
        &mut self,
        kind: u32,
        payload: &[u8],
    ) -> Result<u64, ProtocolError> {
        let ring_len = self.ring_len;
        let space_required_for_payload = align_up_pow2(
            SHM_RECORD_HDR_SIZE + payload.len() as u64,
            RING_ALIGN_SHIFT,
        );
        // this really shouldn't be possible
        ensure!(space_required_for_payload <= ring_len, MisMatchSnafu);

        // calculate metadata for writing first
        let (seq, mut offset_to_new_update) = {
            let hv = self.header_view();
            let prev_seq = hv.write_seq().load(Ordering::Acquire);
            let seq = prev_seq.wrapping_add(1);
            let off =
                hv.write_next_entry_offset().load(Ordering::Acquire) as u64;
            (seq, off)
        };

        let remain = ring_len - offset_to_new_update;

        // if we can't fit the entire thing in, but we can fit the header, then
        // just fit the header
        if space_required_for_payload > remain {
            if remain >= SHM_RECORD_HDR_SIZE {
                let pad_hdr = ShmRecordHeader {
                    payload_kind: Kind::Padding.into(),
                    payload_len: 0,
                    seq,
                };
                self.put_pod_at(offset_to_new_update, &pad_hdr)?;
            }
            std::sync::atomic::fence(Ordering::Release);
            offset_to_new_update = 0;
            // TODO in this path don't I need to increment the seq number again?
        }

        let header = ShmRecordHeader {
            payload_kind: kind,
            payload_len: payload.len() as u32,
            seq,
        };

        // write payload THEN header
        self.put_bytes_at(SHM_RECORD_HDR_SIZE + offset_to_new_update, payload)?;
        // write header
        self.put_pod_at(offset_to_new_update, &header)?;
        std::sync::atomic::fence(Ordering::Release);

        // finally update header
        {
            let hv = self.header_view();
            let next_entry_offset =
                (offset_to_new_update + space_required_for_payload) % ring_len;
            hv.write_next_entry_offset()
                .store(next_entry_offset as u32, Ordering::Release);
            hv.write_seq().store(seq, Ordering::Release);
        }

        Ok(seq)
    }
}
