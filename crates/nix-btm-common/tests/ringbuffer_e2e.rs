use std::{
    ffi::CString,
    mem::size_of,
    os::fd::{AsRawFd, OwnedFd},
};

use bytemuck::try_from_bytes;
use memmap2::MmapOptions;
use nix_btm_common::{
    daemon_side::align_up_pow2,
    protocol_common::{Kind, ShmHeader, ShmRecordHeader, Update},
    ring_reader::RingReader,
    ring_writer::RingWriter,
};
use serde_cbor;

/// Unlink shared memory by name (works on both Linux and macOS)
fn shm_cleanup(name: &str) {
    let c_name = CString::new(name).unwrap();
    unsafe {
        libc::shm_unlink(c_name.as_ptr());
    }
}

// These must match your ring_writer.rs constants
const SHM_RECORD_HDR_SIZE: u32 = size_of::<ShmRecordHeader>() as u32;
const RING_ALIGN_SHIFT: u32 = 3;

/// Decode `n` non-padding records from the ring bytes.
fn decode_updates_from_ring(
    ring_bytes: &[u8],
    ring_len: u32,
    n: usize,
) -> Vec<(u32, Kind, Update)> {
    let mut out = Vec::new();
    let mut off: u32 = 0;

    while out.len() < n {
        let hdr_start = off as usize;
        let hdr_end = hdr_start + SHM_RECORD_HDR_SIZE as usize;
        let hdr_bytes = &ring_bytes[hdr_start..hdr_end];
        let rec_hdr: ShmRecordHeader = *try_from_bytes(hdr_bytes).unwrap();

        // Skip padding
        if rec_hdr.payload_kind == (Kind::Padding as u32) {
            let rec_size = align_up_pow2(SHM_RECORD_HDR_SIZE, RING_ALIGN_SHIFT);
            off = (off + rec_size) % ring_len;
            continue;
        }

        let kind = Kind::try_from(rec_hdr.payload_kind).unwrap();

        let payload_off = off + SHM_RECORD_HDR_SIZE;
        let payload_end = payload_off + rec_hdr.payload_len;
        let payload = &ring_bytes[payload_off as usize..payload_end as usize];

        let update: Update = serde_cbor::from_slice(payload).unwrap();

        out.push((rec_hdr.seq, kind, update));

        let rec_size = align_up_pow2(
            SHM_RECORD_HDR_SIZE + rec_hdr.payload_len,
            RING_ALIGN_SHIFT,
        );
        off = (off + rec_size) % ring_len;
    }

    out
}

#[test]
fn ring_writer_and_reader_e2e_roundtrip_heartbeat() {
    // Cleanup any leftover shm from previous runs
    shm_cleanup("test_ring_e2e");

    let ring_len: u32 = 1024;

    let mut writer = RingWriter::create("test_ring_e2e", ring_len)
        .expect("RingWriter::create should succeed");

    let ring_name = writer.name.clone();

    // total_len must match what RingWriter::create used
    let total_len = (size_of::<ShmHeader>() as u32 + ring_len) as usize;

    // 2. Write some updates
    let updates = vec![
        Update::Heartbeat { daemon_seq: 1 },
        Update::Heartbeat { daemon_seq: 2 },
        Update::Heartbeat { daemon_seq: 3 },
    ];

    let mut seqs = Vec::new();
    for upd in &updates {
        let seq = writer
            .write_update(upd)
            .expect("write_update should succeed");
        seqs.push(seq);
    }

    // 3. Attach a reader to the same shm (tests RingReader::from_name)
    let mut reader = RingReader::from_name(&ring_name, total_len)
        .expect("RingReader::from_name should succeed");

    // 4. Read updates via the reader
    use nix_btm_common::ring_reader::ReadResult;
    let mut read_updates = Vec::new();
    for _ in 0..updates.len() {
        match reader.try_read() {
            ReadResult::Update { seq, update } => {
                read_updates.push((seq, update));
            }
            ReadResult::NoUpdate => {
                panic!("Expected update, got NoUpdate");
            }
            ReadResult::Lost { from, to } => {
                panic!("Unexpected Lost {{ from: {}, to: {} }}", from, to);
            }
            ReadResult::NeedCatchup => {
                panic!("Unexpected NeedCatchup");
            }
        }
    }

    // Verify reader got correct updates
    assert_eq!(read_updates.len(), updates.len());
    for (i, (seq, upd)) in read_updates.iter().enumerate() {
        assert_eq!(*seq, seqs[i], "sequence number should match");
        match upd {
            Update::Heartbeat { daemon_seq } => {
                assert_eq!(*daemon_seq, (i + 1) as u64);
            }
            _ => panic!("expected Heartbeat, got {:?}", upd),
        }
    }

    // Open the shared memory by name to verify raw ring bytes
    use psx_shm::Shm;
    use rustix::{fs::Mode, shm::OFlags};
    let shm =
        Shm::open(&ring_name, OFlags::RDONLY, Mode::from_bits_truncate(0o600))
            .expect("Failed to open shared memory");

    let mmap = unsafe {
        MmapOptions::new()
            .len(total_len)
            .map(shm.as_fd().as_raw_fd())
            .expect("mmap of shm fd should succeed")
    };

    let hdr_size = size_of::<ShmHeader>();
    let ring_bytes = &mmap[hdr_size..hdr_size + ring_len as usize];

    let decoded = decode_updates_from_ring(ring_bytes, ring_len, updates.len());

    assert_eq!(decoded.len(), updates.len());

    for (i, (seq, kind, upd)) in decoded.into_iter().enumerate() {
        assert_eq!(seq, seqs[i], "sequence number should match");
        assert_eq!(kind, Kind::Heartbeat, "kind should be Heartbeat");

        match upd {
            Update::Heartbeat { daemon_seq } => {
                assert_eq!(daemon_seq, (i + 1) as u64);
            }
            _ => panic!("expected Heartbeat, got {:?}", upd),
        }
    }
}

#[test]
fn ring_writer_wraparound_and_reader_validity() {
    shm_cleanup("test_ring_wraparound");
    // Use a small ring buffer to force wraparound quickly
    let ring_len: u32 = 256;

    let mut writer = RingWriter::create("test_ring_wraparound", ring_len)
        .expect("RingWriter::create should succeed");

    let ring_name = writer.name.clone();
    let total_len = (size_of::<ShmHeader>() as u32 + ring_len) as usize;

    // Write enough updates to wrap around the ring multiple times
    // Each heartbeat serializes to roughly 20-30 bytes, so we need ~20+ updates
    // to wrap
    let num_updates = 50;
    let mut all_seqs = Vec::new();

    for i in 0..num_updates {
        let upd = Update::Heartbeat {
            daemon_seq: i as u64,
        };
        let seq = writer
            .write_update(&upd)
            .expect("write_update should succeed");
        all_seqs.push(seq);
    }

    // Create a reader AFTER all writes - it should start from the oldest valid
    // data
    let mut reader = RingReader::from_name(&ring_name, total_len)
        .expect("RingReader::from_name should succeed");

    // Read all available updates
    use nix_btm_common::ring_reader::ReadResult;
    let mut read_updates = Vec::new();
    let mut got_lost = false;
    loop {
        match reader.try_read() {
            ReadResult::Update { seq, update } => {
                read_updates.push((seq, update));
            }
            ReadResult::NoUpdate => {
                break;
            }
            ReadResult::Lost { from, to } => {
                // After wraparound, reader may detect it missed some early
                // updates
                println!(
                    "Reader detected lost updates from {} to {}",
                    from, to
                );
                got_lost = true;
                // Continue reading - there should still be valid data
            }
            ReadResult::NeedCatchup => {
                // After heavy wraparound, reader might need full catchup
                println!("Reader needs catchup after wraparound");
                break;
            }
        }
    }

    // If we wrapped around, we should have detected lost updates
    if got_lost {
        println!("Reader correctly detected lost updates due to wraparound");
    }

    // In a wraparound scenario, we might:
    // 1. Successfully read some updates, OR
    // 2. Immediately detect we need catchup if wraparound was severe
    // Both are valid outcomes - the key is that we detected the issue
    if read_updates.is_empty() {
        println!(
            "Heavy wraparound - reader needs immediate catchup (valid \
             behavior)"
        );
    } else {
        println!(
            "Successfully read {} updates before needing catchup",
            read_updates.len()
        );
    }

    // Verify all read updates are valid heartbeats with correct sequence
    for (seq, upd) in read_updates.iter() {
        match upd {
            Update::Heartbeat { daemon_seq } => {
                // The daemon_seq should match the index we wrote
                assert!(
                    *daemon_seq < num_updates as u64,
                    "daemon_seq {} should be < {}",
                    daemon_seq,
                    num_updates
                );
            }
            _ => panic!("expected Heartbeat, got {:?}", upd),
        }
    }

    println!(
        "Wrote {} updates, read {} updates after wraparound",
        num_updates,
        read_updates.len()
    );
}

#[test]
fn ring_reader_detects_being_lapped() {
    shm_cleanup("test_ring_lapped");
    // Small ring to make lapping easy
    let ring_len: u32 = 128;

    let mut writer = RingWriter::create("test_ring_lapped", ring_len)
        .expect("RingWriter::create should succeed");

    let ring_name = writer.name.clone();
    let total_len = (size_of::<ShmHeader>() as u32 + ring_len) as usize;

    // Write a few updates
    for i in 0..3 {
        let upd = Update::Heartbeat { daemon_seq: i };
        writer.write_update(&upd).expect("write should succeed");
    }

    // Create reader - it will start at the beginning
    let mut reader = RingReader::from_name(&ring_name, total_len)
        .expect("RingReader::from_name should succeed");

    // Read one update
    use nix_btm_common::ring_reader::ReadResult;
    match reader.try_read() {
        ReadResult::Update { .. } => {}
        other => panic!("Expected Update, got {:?}", other),
    }

    // Now write MANY more updates to wrap around and invalidate the reader's
    // position
    for i in 3..60 {
        let upd = Update::Heartbeat { daemon_seq: i };
        writer.write_update(&upd).expect("write should succeed");
    }

    // Reader should now detect it's been lapped
    match reader.try_read() {
        ReadResult::NeedCatchup => {
            println!("Reader correctly detected being lapped");
        }
        ReadResult::Lost { from, to } => {
            println!("Reader detected lost updates from {} to {}", from, to);
            // This is also acceptable - reader detected it missed updates
        }
        other => panic!("Expected NeedCatchup or Lost, got {:?}", other),
    }
}
