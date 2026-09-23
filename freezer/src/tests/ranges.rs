use crate::format::{INDEX_ENTRY_SIZE, IndexEntry, checksum};
use crate::payload::{self, CHUNK_SIZE, CHUNK_THRESHOLD, ChunkTable};
use crate::storage::data_path;
use crate::{ArchivedBlock, Freezer, FreezerFilesBuilder};
use ckb_types::{
    core::{BlockBuilder, TransactionBuilder},
    packed,
    prelude::*,
};
use std::{
    fs::{self, OpenOptions},
    io::{Seek, SeekFrom, Write},
};

#[test]
fn checksum_matches_the_format_algorithm() {
    assert_eq!(
        checksum(b"abc"),
        ckb_types::h256!("0xba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad").0
    );
}

fn bytes(length: usize) -> Vec<u8> {
    let mut state = 17u64;
    (0..length)
        .map(|index| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            if index % 64 < 40 {
                (index % 16) as u8
            } else {
                state as u8
            }
        })
        .collect()
}

fn block(count: usize, witness: usize, uncles: usize) -> packed::Block {
    BlockBuilder::default()
        .transactions((0..count).map(|index| {
            TransactionBuilder::default()
                .version(index as u32)
                .witness(packed::Bytes::from(bytes(witness)))
                .build()
        }))
        .uncles((0..uncles).map(|index| {
            BlockBuilder::default()
                .nonce(index as u128)
                .build()
                .as_uncle()
        }))
        .proposal(packed::ProposalShortId::new([3; 10]))
        .extension(Some(bytes(100).into()))
        .build()
        .data()
}

#[test]
fn ranges_match_full_records_at_codec_and_chunk_boundaries() {
    let directory = tempfile::tempdir().unwrap();
    let records: Vec<_> = [
        0,
        1,
        CHUNK_SIZE - 1,
        CHUNK_SIZE,
        CHUNK_SIZE + 1,
        CHUNK_THRESHOLD - 1,
        CHUNK_THRESHOLD,
        CHUNK_THRESHOLD + 1,
        8 * CHUNK_SIZE + 17,
    ]
    .into_iter()
    .map(bytes)
    .collect();
    {
        let mut files = FreezerFilesBuilder::new(directory.path().to_path_buf())
            .max_file_size(32 * 1024)
            .build()
            .unwrap();
        for (index, raw) in records.iter().enumerate() {
            files.append(index as u64 + 1, raw).unwrap();
        }
        files.sync_all().unwrap();
    }
    let files = FreezerFilesBuilder::new(directory.path().to_path_buf())
        .build()
        .unwrap();
    for (index, raw) in records.iter().enumerate() {
        let record = files.reader.record(index as u64 + 1).unwrap().unwrap();
        assert_eq!(record.retrieve_limited(record.read_bytes()).unwrap(), *raw);
        assert!(record.retrieve_limited(record.read_bytes() - 1).is_err());
        let data = record.ranges().unwrap();
        let positions = [
            0,
            1,
            CHUNK_SIZE - 1,
            CHUNK_SIZE,
            CHUNK_SIZE + 1,
            CHUNK_THRESHOLD - 1,
            CHUNK_THRESHOLD,
            raw.len().saturating_sub(1),
            raw.len(),
        ];
        for start in positions.into_iter().filter(|start| *start <= raw.len()) {
            for end in positions
                .into_iter()
                .filter(|end| *end >= start && *end <= raw.len())
            {
                assert_eq!(
                    &*data.read(start..end).unwrap(),
                    &raw[start..end],
                    "record {index}, {start}..{end}"
                );
            }
        }
        assert!(data.read(0..raw.len() + 1).is_err());
        let reversed = std::ops::Range {
            start: usize::MAX,
            end: 0,
        };
        assert!(data.read(reversed).is_err());
    }
}

#[test]
fn selective_block_fields_match_full_molecule_readers() {
    // Large uncle and transaction counts move the vector and its offset table
    // beyond the first chunk; neither positioning shortcut may be assumed.
    let records = [
        block(0, 0, 0),
        block(3, 512, 1),
        block(16, 8192, 2),
        block(3, 20_000, 48),
        block(2200, 0, 0),
        BlockBuilder::default().build().data(),
    ];
    let directory = tempfile::tempdir().unwrap();
    {
        let mut files = FreezerFilesBuilder::new(directory.path().to_path_buf())
            .build()
            .unwrap();
        for (index, block) in records.iter().enumerate() {
            files.append(index as u64 + 1, block.as_slice()).unwrap();
        }
        files.sync_all().unwrap();
    }
    let freezer = Freezer::open_in(directory.path()).unwrap();
    for (index, expected) in records.iter().enumerate() {
        let ordinal = index as u64 + 1;
        let block = freezer
            .read_block(ordinal, &expected.calc_header_hash())
            .unwrap()
            .unwrap();
        assert_eq!(
            block.uncles().unwrap().as_slice(),
            expected.uncles().as_slice()
        );
        assert_eq!(
            block.proposals().unwrap().as_slice(),
            expected.proposals().as_slice()
        );
        assert_eq!(block.extension().unwrap(), expected.extension());
        for position in [
            0,
            1,
            expected.transactions().len().saturating_sub(1),
            expected.transactions().len(),
            usize::MAX,
        ] {
            let (transaction, count) = block.transaction_with_count(position).unwrap();
            assert_eq!(count, expected.transactions().len());
            assert_eq!(
                transaction.map(|tx| tx.as_bytes()),
                expected
                    .transactions()
                    .get(position)
                    .map(|tx| tx.as_bytes())
            );
            assert_eq!(
                block.transaction(position).unwrap().map(|tx| tx.as_bytes()),
                expected
                    .transactions()
                    .get(position)
                    .map(|tx| tx.as_bytes())
            );
        }
        assert!(
            freezer
                .read_block(ordinal, &packed::Byte32::default())
                .is_err()
        );
    }
    assert!(
        freezer
            .read_block(0, &packed::Byte32::default())
            .unwrap()
            .is_none()
    );
    assert!(
        freezer
            .read_block(records.len() as u64 + 1, &packed::Byte32::default())
            .unwrap()
            .is_none()
    );
}

#[test]
fn every_requested_chunk_is_verified_and_full_reads_detect_unrequested_corruption() {
    let raw = bytes(8 * CHUNK_SIZE + 73);
    let directory = tempfile::tempdir().unwrap();
    let mut files = FreezerFilesBuilder::new(directory.path().to_path_buf())
        .build()
        .unwrap();
    files.append(1, &raw).unwrap();
    files.sync_all().unwrap();
    let path = data_path(directory.path(), 0);
    let original = fs::read(&path).unwrap();
    let encoded = fs::read(directory.path().join("INDEX")).unwrap();
    let entry = IndexEntry::decode(&encoded).unwrap();
    let table = ChunkTable::parse(&original, raw.len(), original.len(), &entry.data_hash).unwrap();
    let mut file = OpenOptions::new().write(true).open(&path).unwrap();
    for (index, (range, _)) in table.chunks().enumerate() {
        file.seek(SeekFrom::Start(range.start as u64)).unwrap();
        file.write_all(&[original[range.start] ^ 0x40]).unwrap();
        assert!(files.retrieve(1).is_err(), "chunk {index}");
        let record = files.reader.record(1).unwrap().unwrap();
        if index == 0 {
            assert!(record.ranges().is_err());
        } else {
            let reader = record.ranges().unwrap();
            assert_eq!(&*reader.read(0..16).unwrap(), &raw[..16]);
            assert!(
                reader
                    .read(index * CHUNK_SIZE..index * CHUNK_SIZE + 1)
                    .is_err()
            );
        }
        file.seek(SeekFrom::Start(range.start as u64)).unwrap();
        file.write_all(&original[range.start..range.start + 1])
            .unwrap();
    }
    // The table hash protects every length and chunk checksum, even when the
    // damaged entry belongs to a chunk the query would not otherwise read.
    for offset in 0..payload::table_len(raw.len()).unwrap() {
        file.seek(SeekFrom::Start(offset as u64)).unwrap();
        file.write_all(&[original[offset] ^ 1]).unwrap();
        assert!(
            files.reader.record(1).unwrap().unwrap().ranges().is_err(),
            "table byte {offset}"
        );
        file.seek(SeekFrom::Start(offset as u64)).unwrap();
        file.write_all(&original[offset..offset + 1]).unwrap();
    }
    files.verify().unwrap();
}

#[test]
fn malformed_checksummed_chunk_tables_and_decoded_lengths_are_rejected() {
    let raw = bytes(CHUNK_THRESHOLD + 1);
    let (original, _) = payload::encode(&raw).unwrap();
    let table_len = payload::table_len(raw.len()).unwrap();
    for length in [0u32, u32::MAX, 1] {
        let mut data = original.clone();
        data[..4].copy_from_slice(&length.to_le_bytes());
        let hash = checksum(&data[..table_len]);
        assert!(ChunkTable::parse(&data, raw.len(), data.len(), &hash).is_err());
    }
    let hash = checksum(&original[..table_len]);
    assert!(
        ChunkTable::parse(&original[..table_len - 1], raw.len(), original.len(), &hash).is_err()
    );
    assert!(ChunkTable::parse(&original, raw.len(), original.len() - 1, &hash).is_err());
    let compressed = lz4_flex::block::compress(b"short");
    let hash = checksum(&compressed);
    assert!(payload::decode_into(&compressed, &hash, &mut [0; 4]).is_err());
    assert!(payload::decode_into(&compressed, &hash, &mut [0; 6]).is_err());
}

#[test]
fn prepared_records_reuse_metadata_and_reject_impossible_sizes_before_data_io() {
    let directory = tempfile::tempdir().unwrap();
    let mut files = FreezerFilesBuilder::new(directory.path().to_path_buf())
        .build()
        .unwrap();
    let raw = bytes(CHUNK_THRESHOLD + 1);
    files.append(1, &raw).unwrap();
    files.sync_all().unwrap();
    let path = directory.path().join("INDEX");
    let original = fs::read(&path).unwrap();
    assert_eq!(original.len(), INDEX_ENTRY_SIZE as usize);
    let record = files.reader.record(1).unwrap().unwrap();
    let mut damaged = original.clone();
    damaged[0] ^= 1;
    fs::write(&path, &damaged).unwrap();
    assert_eq!(record.retrieve_limited(record.read_bytes()).unwrap(), raw);
    assert!(files.retrieve(1).is_err());
    // The index checksum alone does not make an allocation size plausible.
    for (length, stored, offset) in [
        (u64::MAX, 1, 0),
        (1 << 40, 10, 0),
        (100, 10, u64::MAX),
        (100, u32::MAX, 0),
    ] {
        let mut entry = IndexEntry::decode(&original).unwrap();
        entry.raw_len = length;
        entry.stored_len = stored;
        entry.offset = offset;
        fs::write(&path, entry.encode()).unwrap();
        assert!(files.reader.record(1).is_err());
    }
}

#[test]
fn invalid_molecule_layouts_fail_without_panicking_and_invalid_extension_stays_optional() {
    let expected = block(3, 20_000, 0);
    let raw = expected.as_slice();
    let word = |offset| u32::from_le_bytes(raw[offset..offset + 4].try_into().unwrap());
    let transaction_start = word(12) as usize;
    let transaction_end = word(16) as usize;
    for (offset, value, block_error) in [
        (0, raw.len() as u32 - 1, true),
        (4, 22, true),
        (4, 16, true),
        (4, (raw.len() as u32 / 4) * 4, true),
        (8, 23, true),
        (12, raw.len() as u32 + 1, true),
        (transaction_start, 4, false),
        (transaction_start + 4, 6, false),
        (
            transaction_start + 8,
            (transaction_end - transaction_start + 1) as u32,
            false,
        ),
    ] {
        let mut invalid = raw.to_vec();
        invalid[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        let directory = tempfile::tempdir().unwrap();
        let mut files = FreezerFilesBuilder::new(directory.path().to_path_buf())
            .build()
            .unwrap();
        files.append(1, &invalid).unwrap();
        files.sync_all().unwrap();
        let record = files.reader.record(1).unwrap().unwrap();
        let read = ArchivedBlock::new(record, &expected.calc_header_hash());
        if block_error {
            assert!(read.is_err(), "offset {offset}");
        } else {
            assert!(read.unwrap().transaction(0).is_err(), "offset {offset}");
        }
    }
    let mut invalid_header = raw.to_vec();
    for offset in [8, 12, 16, 20] {
        invalid_header[offset..offset + 4].copy_from_slice(&(raw.len() as u32).to_le_bytes());
    }
    let directory = tempfile::tempdir().unwrap();
    let mut files = FreezerFilesBuilder::new(directory.path().to_path_buf())
        .build()
        .unwrap();
    files.append(1, &invalid_header).unwrap();
    files.sync_all().unwrap();
    assert!(
        ArchivedBlock::new(
            files.reader.record(1).unwrap().unwrap(),
            &expected.calc_header_hash()
        )
        .is_err()
    );
    let mut invalid_extension = raw.to_vec();
    let extension = word(20) as usize;
    invalid_extension[extension..extension + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    let expected = packed::Block::from_compatible_slice(&invalid_extension).unwrap();
    assert!(expected.extension().is_none());
    let directory = tempfile::tempdir().unwrap();
    let mut files = FreezerFilesBuilder::new(directory.path().to_path_buf())
        .build()
        .unwrap();
    files.append(1, &invalid_extension).unwrap();
    files.sync_all().unwrap();
    let read = ArchivedBlock::new(
        files.reader.record(1).unwrap().unwrap(),
        &expected.calc_header_hash(),
    )
    .unwrap();
    assert!(read.extension().unwrap().is_none());
    assert_eq!(
        read.transaction(1).unwrap().map(|tx| tx.as_bytes()),
        expected.transactions().get(1).map(|tx| tx.as_bytes())
    );
}
