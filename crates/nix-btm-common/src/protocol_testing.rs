#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet, HashMap},
        fs::File,
        io::{Read, Seek, SeekFrom},
        os::{
            fd::{AsFd, OwnedFd},
            unix::io::{AsRawFd, FromRawFd},
        },
        sync::Once,
    };

    use color_eyre::eyre::Context;
    use rustix::{
        io::dup,
        mm::{MapFlags, ProtFlags, mmap, munmap},
    };
    use snafu::{ErrorCompat, Report};
    use tracing_subscriber::EnvFilter;

    static INIT: Once = Once::new();

    pub fn test_setup() {
        INIT.call_once(|| {
            let _ = color_eyre::config::HookBuilder::new()
                .capture_span_trace_by_default(false)
                .display_env_section(true)
                .add_default_filters()
                .panic_section("custom panic info")
                .install();
            let _ = tracing_subscriber::fmt::try_init();
        });
    }
    use color_eyre::Section;

    use super::*;
    use crate::{
        client_side::client_read_snapshot_into_state,
        daemon_side::create_shmem_and_write_snapshot,
        derivation_tree::{DrvNode, DrvRelations},
        handle_internal_json::{BuildJob, Drv, JobId, JobsStateInner},
        protocol_common::{JobsStateInnerWire, SnapshotHeader},
    };
    fn to_eyre_with_origin_bt<
        E: std::error::Error + Send + Sync + 'static + snafu::ErrorCompat,
    >(
        e: E,
    ) -> color_eyre::Report {
        let bt_str = ErrorCompat::backtrace(&e)
            .map(|bt| format!("{bt:#?}"))
            .unwrap_or_else(|| "no captured backtrace on error".to_string());

        color_eyre::eyre::eyre!(e).with_section(|| {
            color_eyre::SectionExt::header(
                bt_str,
                "Origin backtrace (from error)",
            )
        })
    }

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
    fn e2e_snapshot_round_trip() -> eyre::Result<()> {
        test_setup();

        let state_in = make_min_state();
        let snap_seq_uid = 12345;

        let mem = create_shmem_and_write_snapshot(
            &state_in,
            snap_seq_uid,
            std::process::id() as i32,
        )
        .map_err(to_eyre_with_origin_bt)
        .wrap_err("top-level failure")?;

        let dup_fd =
            dup(mem.shmem.shm.as_fd()).expect("Unable to duplicate fd");
        let mut file = File::from(dup_fd);

        let mut hdr_bytes = vec![0u8; core::mem::size_of::<SnapshotHeader>()];
        file.read_exact(&mut hdr_bytes).expect("issue reading file");
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

        let state_out = client_read_snapshot_into_state(
            mem.shmem.shm.name(),
            mem.total_len_bytes,
        )
        .map_err(to_eyre_with_origin_bt)
        .wrap_err("top-level failure")?;

        let got_wire: JobsStateInnerWire = state_out.into();
        assert_eq!(got_wire.jobs, expected_wire.jobs, "jobs mismatch");
        assert_eq!(got_wire.nodes, expected_wire.nodes, "nodes mismatch");
        assert_eq!(got_wire.roots, expected_wire.roots, "roots mismatch");
        Ok(())
    }
}
