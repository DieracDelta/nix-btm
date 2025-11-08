use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    ptr::copy_nonoverlapping,
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
};

use bytemuck::{Pod, Zeroable, bytes_of};
use memmap2::MmapMut;
use psx_shm::Shm;
use rustix::{fs::Mode, shm::OFlags};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, ensure};

use crate::{
    daemon_comms::{
        CborSnafu, IoSnafu, MisMatchSnafu, ProtocolError, align_up_pow2,
    },
    derivation_tree::{DrvNode, DrvRelations},
    handle_internal_json::{
        BuildJob, Drv, DrvParseError, JobId, JobsStateInner,
    },
};

/// how much to shift to get alignment
const RING_ALIGN_SHIFT: u32 = 3;
const SHM_HDR_SIZE: u64 = size_of::<ShmHeader>() as u64;
const SHM_HDR_SIZE_ALIGNED: u64 = align_up_pow2(SHM_HDR_SIZE, RING_ALIGN_SHIFT);
const SHM_RECORD_HDR_SIZE: u64 = size_of::<ShmRecordHeader>() as u64;
const SHM_RECORD_HDR_SIZE_ALIGNED: u64 =
    align_up_pow2(SHM_RECORD_HDR_SIZE, RING_ALIGN_SHIFT);

// needed because drv serialization is already done differently to accomodate
// for json. Don't need that for cbor
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum DrvWire {
    Str(String),
    Parts { hash: String, name: String },
}

impl From<Drv> for DrvWire {
    fn from(d: Drv) -> Self {
        DrvWire::Parts {
            hash: d.hash,
            name: d.name,
        }
    }
}
impl TryFrom<DrvWire> for Drv {
    type Error = DrvParseError;

    fn try_from(w: DrvWire) -> Result<Self, Self::Error> {
        match w {
            DrvWire::Str(s) => s.parse(), // via FromStr
            DrvWire::Parts { hash, name } => Ok(Drv { hash, name }),
        }
    }
}

// there's two things: a catchup protocol and a update protocol

#[repr(C)]
#[derive(Clone, Copy, Debug, Zeroable, Pod)]
pub struct SnapshotHeader {
    pub magic: u64,
    pub version: u64,
    pub header_len: u64,
    pub payload_len: u64,  // CBOR blob length
    pub snap_seq_uid: u64, // ring seq num at snapshot time
}

impl SnapshotHeader {
    pub const MAGIC: u64 = u64::from_be_bytes(*b"FOOBAR42");
    pub const VERSION: u64 = 1;

    pub fn new(payload_len: u64, snap_seq_uid: u64) -> Self {
        Self {
            magic: Self::MAGIC,
            version: Self::VERSION,
            header_len: std::mem::size_of::<SnapshotHeader>() as u64,
            payload_len,
            snap_seq_uid,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Zeroable, Pod)]
pub struct ShmHeader {
    pub magic: u64,
    pub version: u64,
    pub write_seq: u64,
    pub next_entry_offset: u32,
    pub ring_len: u32,
}

/// Fixed-size prefix of each record in the ring buffer
#[repr(C)]
#[derive(Clone, Copy, Zeroable, Pod)]
pub struct ShmRecordHeader {
    pub payload_kind: u32, // Kind enum as numeric value
    pub payload_len: u32,  // length of CBOR payload
    pub seq: u64,          // sequence number
}

impl ShmHeader {
    pub const MAGIC: u64 = u64::from_be_bytes(*b"FOOBAR42");
    pub const VERSION: u64 = 1;
}

struct ShmHeaderViewMut<'a> {
    hdr: &'a mut ShmHeader,
}

impl<'a> ShmHeaderViewMut<'a> {
    fn new(hdr: &'a mut ShmHeader) -> Self {
        Self { hdr }
    }

    #[inline]
    fn write_seq(&self) -> &AtomicU64 {
        unsafe {
            &*(std::ptr::addr_of!(self.hdr.write_seq) as *const AtomicU64)
        }
    }
    #[inline]
    fn write_next_entry_offset(&self) -> &AtomicU32 {
        unsafe {
            &*(std::ptr::addr_of!(self.hdr.next_entry_offset)
                as *const AtomicU32)
        }
    }

    #[inline]
    fn ring_len(&self) -> u32 {
        self.hdr.ring_len
    }
    #[inline]
    fn magic(&self) -> u64 {
        self.hdr.magic
    }
    #[inline]
    fn version(&self) -> u64 {
        self.hdr.version
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Kind {
    Padding = 0,
    JobNew = 1,
    JobUpdate = 2,
    JobFinish = 3,
    DepGraphUpdate = 4,
    Heartbeat = 5,
}

impl From<Kind> for u32 {
    fn from(value: Kind) -> Self {
        value as u32
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Update {
    JobNew(BuildJob),
    JobUpdate { jid: u64, status: String },
    JobFinish { jid: u64, stop_time_ns: u64 },
    DepGraphUpdate { drv: Drv, deps: Vec<Drv> },
    Heartbeat { daemon_seq: u64 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobsStateInnerWire {
    pub jobs: Vec<BuildJob>,
    pub nodes: Vec<(Drv, DrvNode)>,
    pub roots: Vec<Drv>,
}
impl From<JobsStateInner> for JobsStateInnerWire {
    fn from(state: JobsStateInner) -> Self {
        let mut jobs: Vec<_> = state.jid_to_job.into_values().collect();
        jobs.sort_by_key(|j| j.jid);

        let nodes: Vec<(Drv, DrvNode)> =
            state.dep_tree.nodes.into_iter().collect();

        let roots: Vec<Drv> = state.dep_tree.tree_roots.into_iter().collect();

        JobsStateInnerWire { jobs, nodes, roots }
    }
}
impl From<JobsStateInnerWire> for JobsStateInner {
    fn from(wire: JobsStateInnerWire) -> Self {
        let mut jid_to_job: HashMap<JobId, BuildJob> =
            HashMap::with_capacity(wire.jobs.len());
        for j in wire.jobs {
            jid_to_job.insert(j.jid, j);
        }

        let mut drv_to_jobs: HashMap<Drv, HashSet<JobId>> = HashMap::new();
        for (jid, job) in jid_to_job.iter() {
            drv_to_jobs.entry(job.drv.clone()).or_default().insert(*jid);
        }

        let nodes_map: BTreeMap<Drv, DrvNode> =
            wire.nodes.into_iter().collect();
        let roots_set: BTreeSet<Drv> = wire.roots.into_iter().collect();
        let dep_tree = DrvRelations {
            nodes: nodes_map,
            tree_roots: roots_set,
        };

        JobsStateInner {
            jid_to_job,
            drv_to_jobs,
            dep_tree,
        }
    }
}

pub const RW_MODE: Mode = Mode::from_bits_retain(0o600);
pub const R_MODE: Mode = Mode::from_bits_retain(0o400);

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
            crate::daemon_comms::MisMatchSnafu
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
