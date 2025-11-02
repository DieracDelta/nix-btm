use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};

use crate::{
    derivation_tree::{DrvNode, DrvRelations},
    handle_internal_json::{
        BuildJob, Drv, DrvParseError, JobId, JobsStateInner,
    },
};

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
    pub magic: u64,     // "FOOBAR42"
    pub version: u64,   // schema version
    pub write_seq: u64, // global write counter (monotonic)
    pub write_off: u32, // byte offset in ring where next write begins
    pub ring_len: u32,  // total length of ring region (after header)
}

/// Fixed-size prefix of each record in the ring buffer
#[repr(C)]
#[derive(Clone, Copy, Zeroable, Pod)]
pub struct ShmRecordHeader {
    pub kind: u32, // Kind enum as numeric value
    pub len: u32,  // length of CBOR payload
    pub seq: u64,  // sequence number
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Kind {
    JobNew = 1,
    JobUpdate = 2,
    JobFinish = 3,
    DepGraphUpdate = 4,
    Heartbeat = 5,
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
