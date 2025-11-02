use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};

use crate::handle_internal_json::{BuildJob, Drv, DrvParseError};

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
pub const SNAPSHOT_MAGIC: u64 = u64::from_be_bytes(*b"FOOBAR42");
pub const SNAPSHOT_VERSION: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Zeroable, Pod)]
pub struct SnapshotHeader {
    pub magic: u64,    // "NBTMSNAP"
    pub snap_seq: u64, // ring seq num at snapshot time

    // Offsets/lengths from start-of-file, bytes (< 4 GiB)
    pub off_jobs: u64, // CBOR: Vec<BuildJob>
    pub len_jobs: u64,
    pub off_nodes: u64, // CBOR: Vec<(Drv, DrvNode)>
    pub len_nodes: u64,
    pub off_roots: u64, // CBOR: Vec<Drv>
    pub len_roots: u64,
    pub version: u64, // header/schema version
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
