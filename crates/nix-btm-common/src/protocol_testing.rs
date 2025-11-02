#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet, HashMap},
        fs::File,
        io::{Read, Seek, SeekFrom},
        os::{
            fd::AsFd,
            unix::io::{AsRawFd, FromRawFd},
        },
    };

    use rustix::{
        io::dup,
        mm::{MapFlags, ProtFlags, mmap, munmap},
    };

    use super::*;
    use crate::{
        client_comms::client_read_snapshot_into_state,
        client_daemon_comms::{JobsStateInnerWire, SnapshotHeader},
        daemon_comms::create_shmem_and_write_snapshot,
        derivation_tree::{DrvNode, DrvRelations},
        handle_internal_json::{BuildJob, Drv, JobId, JobsStateInner},
    };

    fn make_min_state() -> JobsStateInner {
        let drv = Drv {
            name: "libdeflate-1.24".to_string(),
            hash: "l6rvxc2nsgqv9416xqkhf0ygar7ycn75".to_string(),
        };
        let jid = JobId(1);
        let job = BuildJob {
            jid,
            drv: drv.clone(),
            rid: Default::default(),
            status: Default::default(),
            start_time_ns: 0,
            stop_time_ns: Some(0),
        };
        let mut jid_to_job: HashMap<JobId, BuildJob> = HashMap::new();
        jid_to_job.insert(jid, job);

        let mut nodes: BTreeMap<Drv, DrvNode> = BTreeMap::new();
        nodes.insert(drv.clone(), Default::default());

        let mut roots: BTreeSet<Drv> = BTreeSet::new();
        roots.insert(drv);

        JobsStateInner {
            jid_to_job,
            drv_to_jobs: Default::default(),
            dep_tree: DrvRelations {
                nodes,
                tree_roots: roots,
            },
        }
    }
    #[test]
    fn e2e_snapshot_round_trip() {
        let state_in = make_min_state();
        let snap_seq_uid = 12345;

        let mem = create_shmem_and_write_snapshot(
            &state_in,
            snap_seq_uid,
            std::process::id() as i32,
        )
        .expect("snapshot creation failed");

        let dup_fd = dup(mem.fd.as_fd()).expect("dup");
        let mut file = File::from(dup_fd);

        let mut hdr_bytes = vec![0u8; core::mem::size_of::<SnapshotHeader>()];
        file.read_exact(&mut hdr_bytes).expect("read header");
        let hdr: &SnapshotHeader = bytemuck::from_bytes(&hdr_bytes);

        assert_eq!(hdr.magic, SnapshotHeader::MAGIC, "bad magic");
        assert_eq!(hdr.version, SnapshotHeader::VERSION, "bad version");
        assert_eq!(
            hdr.header_len,
            core::mem::size_of::<SnapshotHeader>() as u64,
            "unexpected header_len"
        );
        assert_eq!(hdr.snap_seq_uid, snap_seq_uid, "snap_seq_uid mismatch");
        assert!(hdr.payload_len > 0, "payload_len should be > 0");

        file.seek(SeekFrom::Start(hdr.header_len))
            .expect("seek to payload");
        let mut first_byte = [0u8; 1];
        file.read_exact(&mut first_byte)
            .expect("read first payload byte");

        let expected_wire: JobsStateInnerWire = state_in.into();
        let expected_payload =
            serde_cbor::to_vec(&expected_wire).expect("encode");
        assert_eq!(
            first_byte[0], expected_payload[0],
            "payload does not start at header_len"
        );

        let state_out =
            client_read_snapshot_into_state(&mem.fd, mem.total_len_bytes)
                .expect("client_read_snapshot_into_state failed");

        let got_wire: JobsStateInnerWire = state_out.into();
        assert_eq!(got_wire.jobs, expected_wire.jobs, "jobs mismatch");
        assert_eq!(got_wire.nodes, expected_wire.nodes, "nodes mismatch");
        assert_eq!(got_wire.roots, expected_wire.roots, "roots mismatch");
    }

    #[test]
    fn e2e_client_rejects_bad_magic() {
        let state_in = make_min_state();
        let snap_seq_uid = 1;

        let mem = create_shmem_and_write_snapshot(
            &state_in,
            snap_seq_uid,
            std::process::id() as i32,
        )
        .expect("snapshot creation failed");

        // Corrupt the magic byte
        let base = unsafe {
            mmap(
                core::ptr::null_mut(),
                mem.total_len_bytes as usize,
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::SHARED,
                &mem.fd,
                0,
            )
            .expect("mmap rw") as *mut u8
        };
        let buf = unsafe {
            core::slice::from_raw_parts_mut(base, mem.total_len_bytes as usize)
        };

        buf[0] ^= 0xFF;

        let got = client_read_snapshot_into_state(&mem.fd, mem.total_len_bytes);
        assert!(got.is_none(), "client must reject bad magic");

        // cleanup
        unsafe {
            let _ = munmap(buf.as_mut_ptr() as *mut _, buf.len());
        }
    }
}
