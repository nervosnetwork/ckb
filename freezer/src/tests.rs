use crate::FreezerFilesBuilder;
use crate::format::{Commit, INDEX_ENTRY_SIZE, IndexEntry};
use crate::freezer_files::FreezerFiles;
use crate::storage::data_path;
use std::fs::{self, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::time::Duration;

mod durability;
mod ranges;

fn open(path: &Path) -> FreezerFiles {
    FreezerFilesBuilder::new(path.to_path_buf())
        .max_file_size(400)
        .open_files_limit(2)
        .build()
        .unwrap()
}

fn data(number: u64) -> Vec<u8> {
    let mut state = number + 1;
    (0..(number as usize % 257 + 128))
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn append(files: &mut FreezerFiles, end: u64) {
    for number in files.number()..end {
        files.append(number, &data(number)).unwrap();
    }
}

fn assert_prefix(files: &FreezerFiles, end: u64) {
    assert_eq!(files.committed_number(), end);
    assert_eq!(files.retrieve(0).unwrap(), None);
    assert_eq!(files.retrieve(end).unwrap(), None);
    for number in 1..end {
        assert_eq!(
            files.retrieve(number).unwrap(),
            Some(data(number)),
            "item {number}"
        );
    }
    for number in (1..end).rev() {
        assert_eq!(files.retrieve(number).unwrap(), Some(data(number)));
    }
}

#[test]
fn commit_visibility_and_repeated_reopen() {
    let directory = tempfile::tempdir().unwrap();
    for round in 0..5 {
        let mut files = open(directory.path());
        assert_prefix(&files, round * 20 + 1);
        let end = (round + 1) * 20 + 1;
        append(&mut files, end);
        assert_eq!(files.number(), end);
        assert_prefix(&files, round * 20 + 1);
        files.sync_all().unwrap();
        assert_prefix(&files, end);
    }
    let files = open(directory.path());
    assert_prefix(&files, 101);
    files.verify().unwrap();
}

#[test]
fn lz4_empty_compressible_and_large_records() {
    let directory = tempfile::tempdir().unwrap();
    let records = [Vec::new(), vec![7; 15], vec![42; 2 * 1024 * 1024], data(43)];
    {
        let mut files = open(directory.path());
        for (index, bytes) in records.iter().enumerate() {
            files.append(index as u64 + 1, bytes).unwrap();
        }
        files.sync_all().unwrap();
    }
    let files = open(directory.path());
    for (index, bytes) in records.iter().enumerate() {
        assert_eq!(
            files.retrieve(index as u64 + 1).unwrap().as_ref(),
            Some(bytes)
        );
    }
}

#[test]
fn an_oversized_read_is_rejected_before_opening_or_allocating_payload_buffers() {
    let directory = tempfile::tempdir().unwrap();
    let mut files = open(directory.path());
    let bytes = vec![7; 2 << 20];
    files.append(1, &bytes).unwrap();
    files.sync_all().unwrap();
    let required = files.reader.record(1).unwrap().unwrap().read_bytes();
    assert!(required > bytes.len());
    let data_file = data_path(directory.path(), 0);
    let held_file = directory.path().join("held-payload");
    fs::rename(&data_file, &held_file).unwrap();
    let error = files.reader.retrieve_limited(1, 1024).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(files.committed_number(), 2);
    fs::rename(held_file, data_file).unwrap();
    assert_eq!(
        files.reader.retrieve_limited(1, required).unwrap(),
        Some(bytes)
    );
}

#[test]
fn reading_earlier_items_cannot_reposition_the_writer() {
    let directory = tempfile::tempdir().unwrap();
    {
        let mut files = open(directory.path());
        append(&mut files, 5);
        files.sync_all().unwrap();
        assert_eq!(files.retrieve(1).unwrap(), Some(data(1)));
        append(&mut files, 8);
        files.sync_all().unwrap();
        assert_prefix(&files, 8);
    }
    assert_prefix(&open(directory.path()), 8);
}

#[test]
fn pending_tail_is_discarded_across_many_rotations() {
    let directory = tempfile::tempdir().unwrap();
    {
        let mut files = open(directory.path());
        append(&mut files, 8);
        files.sync_all().unwrap();
        append(&mut files, 40);
        // Dropping is deliberately not a durability acknowledgement.
    }
    {
        let mut files = open(directory.path());
        assert_prefix(&files, 8);
        append(&mut files, 30);
        files.sync_all().unwrap();
    }
    assert_prefix(&open(directory.path()), 30);
}

#[test]
fn torn_uncommitted_index_and_data_are_ignored() {
    let directory = tempfile::tempdir().unwrap();
    {
        let mut files = open(directory.path());
        append(&mut files, 4);
        files.sync_all().unwrap();
    }
    let commit = crate::storage::open_commit(directory.path()).unwrap();
    OpenOptions::new()
        .append(true)
        .open(directory.path().join("INDEX"))
        .unwrap()
        .write_all(&[0xff; 37])
        .unwrap();
    OpenOptions::new()
        .append(true)
        .open(data_path(directory.path(), commit.file_id))
        .unwrap()
        .write_all(&[0xff; 71])
        .unwrap();
    let mut files = open(directory.path());
    assert_prefix(&files, 4);
    append(&mut files, 9);
    files.sync_all().unwrap();
    assert_prefix(&files, 9);
}

#[test]
fn committed_truncation_is_an_error_without_repair() {
    for kind in ["INDEX", "head", "sealed"] {
        let directory = tempfile::tempdir().unwrap();
        {
            let mut files = open(directory.path());
            append(&mut files, 9);
            files.sync_all().unwrap();
        }
        let commit = crate::storage::open_commit(directory.path()).unwrap();
        let path = match kind {
            "INDEX" => directory.path().join("INDEX"),
            "head" => data_path(directory.path(), commit.file_id),
            _ => data_path(directory.path(), 0),
        };
        let file = OpenOptions::new().write(true).open(&path).unwrap();
        let length = file.metadata().unwrap().len() - 1;
        file.set_len(length).unwrap();
        let before = fs::read(directory.path().join("COMMIT")).unwrap();
        let error = FreezerFilesBuilder::new(directory.path().to_path_buf())
            .build()
            .err()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{kind}: {error}");
        assert_eq!(fs::metadata(path).unwrap().len(), length);
        assert_eq!(fs::read(directory.path().join("COMMIT")).unwrap(), before);
    }
}

#[test]
fn missing_committed_files_are_never_recreated() {
    for name in ["COMMIT", "INDEX", "blk000000"] {
        let directory = tempfile::tempdir().unwrap();
        {
            let mut files = open(directory.path());
            append(&mut files, 3);
            files.sync_all().unwrap();
        }
        let path = directory.path().join(name);
        fs::remove_file(&path).unwrap();
        assert!(
            FreezerFilesBuilder::new(directory.path().to_path_buf())
                .build()
                .is_err()
        );
        assert!(!path.exists(), "recreated {name}");
    }
}

#[test]
fn damaged_metadata_is_rejected_at_every_byte() {
    let entry = IndexEntry {
        number: 1,
        file_id: 0,
        stored_len: 3,
        offset: 0,
        raw_len: 2,
        data_hash: [7; 32],
    };
    let encoded = entry.encode();
    assert_eq!(encoded.len() as u64, INDEX_ENTRY_SIZE);
    for byte in 0..encoded.len() {
        let mut damaged = encoded.clone();
        damaged[byte] ^= 1;
        assert!(IndexEntry::decode(&damaged).is_err());
    }
    let encoded = Commit::default().encode();
    for byte in 0..encoded.len() {
        let mut damaged = encoded.clone();
        damaged[byte] ^= 1;
        assert!(Commit::decode(&damaged).is_err());
    }
    for length in 0..96 {
        assert!(IndexEntry::decode(&vec![0; length]).is_err());
        assert!(Commit::decode(&vec![0; length]).is_err());
    }
}

#[test]
fn payload_corruption_is_not_reported_as_missing() {
    let directory = tempfile::tempdir().unwrap();
    let mut files = open(directory.path());
    append(&mut files, 4);
    files.sync_all().unwrap();
    let path = data_path(directory.path(), 0);
    let mut file = OpenOptions::new().write(true).open(path).unwrap();
    file.seek(SeekFrom::Start(2)).unwrap();
    file.write_all(&[0xff; 5]).unwrap();
    let error = files.retrieve(1).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(files.verify().is_err());
}

#[test]
fn reader_outlives_writer_and_keeps_exclusive_recovery_lock() {
    let directory = tempfile::tempdir().unwrap();
    let mut files = open(directory.path());
    append(&mut files, 4);
    files.sync_all().unwrap();
    let reader = Arc::clone(&files.reader);
    drop(files);
    assert!(
        FreezerFilesBuilder::new(directory.path().to_path_buf())
            .build()
            .is_err()
    );
    assert_eq!(reader.retrieve(2).unwrap(), Some(data(2)));
    drop(reader);
    assert_prefix(&open(directory.path()), 4);
}

#[test]
fn concurrent_positional_reads_do_not_wait_for_writer() {
    let directory = tempfile::tempdir().unwrap();
    let mut files = open(directory.path());
    append(&mut files, 12);
    files.sync_all().unwrap();
    let reader = Arc::clone(&files.reader);
    let (pending_tx, pending_rx) = mpsc::channel();
    let (commit_tx, commit_rx) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        append(&mut files, 30);
        pending_tx.send(()).unwrap();
        commit_rx.recv().unwrap();
        files.sync_all().unwrap();
        files
    });
    pending_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let (done_tx, done_rx) = mpsc::channel();
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let reader = Arc::clone(&reader);
            let done_tx = done_tx.clone();
            std::thread::spawn(move || {
                for _ in 0..10 {
                    for number in (1..12).rev() {
                        assert_eq!(reader.retrieve(number).unwrap(), Some(data(number)));
                    }
                    assert_eq!(reader.retrieve(12).unwrap(), None);
                }
                done_tx.send(()).unwrap();
            })
        })
        .collect();
    for _ in 0..workers.len() {
        done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    commit_tx.send(()).unwrap();
    for worker in workers {
        worker.join().unwrap();
    }
    let files = writer.join().unwrap();
    assert_prefix(&files, 30);
    assert_eq!(reader.retrieve(29).unwrap(), Some(data(29)));
}

#[test]
fn invalid_arguments_do_not_poison_writer() {
    let directory = tempfile::tempdir().unwrap();
    let mut files = open(directory.path());
    assert!(files.append(0, b"invalid").is_err());
    assert!(files.append(2, b"invalid").is_err());
    files.append(1, &data(1)).unwrap();
    files.sync_all().unwrap();
    assert_prefix(&files, 2);
    drop(files);
    assert!(
        FreezerFilesBuilder::new(directory.path().to_path_buf())
            .max_file_size(0)
            .build()
            .is_err()
    );
    assert!(
        FreezerFilesBuilder::new(directory.path().to_path_buf())
            .open_files_limit(0)
            .build()
            .is_err()
    );
}

fn blocks() -> Vec<ckb_types::core::BlockView> {
    use ckb_types::core::{BlockBuilder, EpochNumberWithFraction};
    let builder = BlockBuilder::default().epoch(EpochNumberWithFraction::new(1, 0, 100));
    let first = builder.clone().number(1).build();
    let second = builder.clone().number(2).parent_hash(first.hash()).build();
    let third = builder.number(3).parent_hash(second.hash()).build();
    vec![first, second, third]
}

#[test]
fn public_reads_complete_while_append_holds_the_writer_lock() {
    use ckb_types::prelude::*;
    let directory = tempfile::tempdir().unwrap();
    let freezer = crate::Freezer::open(directory.path().join("archive")).unwrap();
    let blocks = blocks();
    freezer
        .append_blocks(&[blocks[0].data(), blocks[1].data()])
        .unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let writer = freezer.clone();
    let third = blocks[2].clone();
    let handle = std::thread::spawn(move || {
        writer.with_writer(|files| {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            files.append(files.number(), third.data().as_slice())?;
            files.sync_all()
        })
    });
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let (read_tx, read_rx) = mpsc::channel();
    let reader = freezer.clone();
    let read_handle = std::thread::spawn(move || read_tx.send(reader.retrieve(1)).unwrap());
    let result = read_rx.recv_timeout(Duration::from_secs(5));
    // Release even on failure so this regression cannot hang the test process.
    release_tx.send(()).unwrap();
    assert_eq!(
        result.unwrap().unwrap(),
        Some(blocks[0].data().as_slice().to_vec())
    );
    read_handle.join().unwrap();
    handle.join().unwrap().unwrap();
    assert_eq!(freezer.number(), 4);
}

#[test]
fn archive_ordinals_accept_distinct_branches_and_arbitrary_heights() {
    use ckb_types::{
        core::{BlockBuilder, EpochNumberWithFraction},
        prelude::*,
    };
    let directory = tempfile::tempdir().unwrap();
    let freezer = crate::Freezer::open_in(directory.path()).unwrap();
    let builder = BlockBuilder::default().epoch(EpochNumberWithFraction::new(1, 0, 100));
    let blocks = [
        builder.clone().number(42).nonce(1).build().data(),
        builder.clone().number(42).nonce(2).build().data(),
        builder.number(7).build().data(),
    ];
    assert_eq!(freezer.append_blocks(&blocks).unwrap(), 1..4);
    drop(freezer);
    let freezer = crate::Freezer::open_in(directory.path()).unwrap();
    for (index, block) in blocks.iter().enumerate() {
        assert_eq!(
            freezer.retrieve(index as u64 + 1).unwrap().unwrap(),
            block.as_slice()
        );
    }
}

#[test]
fn reads_overlap_repeated_append_rotation_and_publication() {
    let directory = tempfile::tempdir().unwrap();
    let mut files = open(directory.path());
    append(&mut files, 4);
    files.sync_all().unwrap();
    let reader = Arc::clone(&files.reader);
    let start = Arc::new(std::sync::Barrier::new(5));
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let reader = Arc::clone(&reader);
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                loop {
                    let end = reader.number();
                    for number in (1..end).rev() {
                        assert_eq!(reader.retrieve(number).unwrap(), Some(data(number)));
                    }
                    if end == 100 {
                        break;
                    }
                }
            })
        })
        .collect();
    start.wait();
    for end in (8..100).step_by(4).chain([100]) {
        append(&mut files, end);
        files.sync_all().unwrap();
    }
    for reader in readers {
        reader.join().unwrap();
    }
    assert_prefix(&files, 100);
}
