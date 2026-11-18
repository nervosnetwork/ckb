//! Consumer fixtures using the real archive publisher and payload collector.

use ckb_db_schema::*;
use ckb_freezer::{FreezerController, FreezerFilesBuilder, FreezerServiceConfig};
use ckb_store::{ChainDB, ChainStore};
use ckb_types::{core::BlockView, packed, prelude::*};
use std::{
    any::Any,
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

/// Publish verified blocks, rotate hot payloads, and prove cold-only routing.
/// The fixture bypasses scheduling/retention, never publication or collection.
pub async fn archive_and_collect(store: &ChainDB, blocks: &[BlockView]) {
    let controller = FreezerController::start(
        store.freezer().unwrap().clone(),
        tokio::runtime::Handle::current(),
        FreezerServiceConfig::default(),
    )
    .unwrap();
    controller
        .append(blocks.iter().map(BlockView::data).collect())
        .await
        .unwrap();
    if let Some(active) = store.db().epoch_segments().active() {
        // The short consensus fixture deliberately advances the maintenance
        // boundary. Use the normal publisher and verify every physical member.
        store.recover_archive().unwrap();
        let next = active + store.db().epoch_segments().width();
        store.db().open_epoch_segment(next).unwrap();
        let result = store
            .collect_epoch_segments(&controller, || next, || false, || {})
            .await
            .unwrap();
        assert_eq!(result.dropped_segments, 1);
        assert_eq!(store.db().epoch_segments().windows(), &[next]);
        assert_cold(store, blocks);
    } else {
        collect_published_archive(store, blocks);
    }
    controller.shutdown().await.unwrap();
}

/// Map committed files, collect the matching hot payloads, and verify both views.
pub fn collect_published_archive(store: &ChainDB, blocks: &[BlockView]) {
    assert_eq!(
        store.recover_archive().unwrap(),
        store.freezer().unwrap().number()
    );
    let result = store.collect_archive(&Default::default(), || {}).unwrap();
    assert!(result.archived > 0, "collection must remove hot payload");
    assert_cold(store, blocks);
    for block in blocks {
        assert_eq!(store.get_block(&block.hash()).as_ref(), Some(block));
    }
}

/// Check raw active-generation keys, independently of cold-aware getters.
/// This does not warm the archive reader's file cache.
pub fn assert_cold(store: &ChainDB, blocks: &[BlockView]) {
    let segments = store.db().epoch_segments();
    for block in blocks {
        let hash = block.hash();
        let route = store.get(COLUMN_BLOCK_ARCHIVE, hash.as_slice()).unwrap();
        // The index contains a nonzero archive record and optional witness hashes.
        assert_eq!(route.len() % 32, 8);
        assert_ne!(&route[..8], &0u64.to_le_bytes());
        for &start in segments.windows() {
            assert!(
                store
                    .db()
                    .segment_get(start, hash.as_slice())
                    .unwrap()
                    .is_none()
            );
        }
        for (column, key) in payload_keys(block) {
            // The segmented layout retains the height/hash index and genesis.
            let retained = segments.active().is_some()
                && (column == COLUMN_NUMBER_HASH
                    || (block.number() == 0
                        && (column != COLUMN_BLOCK_EXTENSION || block.extension().is_some())));
            assert_eq!(
                store.get(column, &key).is_some(),
                retained,
                "fixed {column}"
            );
        }
    }
}

/// Prove the independent reference retains its payload and has no archive route.
pub fn assert_hot(store: &ChainDB, blocks: &[BlockView]) {
    assert!(store.freezer().is_none());
    for block in blocks {
        assert!(
            store
                .get(COLUMN_BLOCK_ARCHIVE, block.hash().as_slice())
                .is_none()
        );
        for (column, key) in payload_keys(block) {
            let present = column != COLUMN_BLOCK_EXTENSION || block.extension().is_some();
            assert_eq!(store.get(column, &key).is_some(), present, "hot {column}");
        }
    }
}

fn payload_keys(block: &BlockView) -> Vec<(Col, Vec<u8>)> {
    let hash = block.hash();
    let mut keys = vec![
        (COLUMN_BLOCK_UNCLE, hash.as_slice().to_vec()),
        (COLUMN_BLOCK_PROPOSAL_IDS, hash.as_slice().to_vec()),
        (COLUMN_BLOCK_EXTENSION, hash.as_slice().to_vec()),
        (
            COLUMN_NUMBER_HASH,
            packed::NumberHash::new_builder()
                .number(block.number())
                .block_hash(hash.clone())
                .build()
                .as_slice()
                .to_vec(),
        ),
    ];
    keys.extend((0..block.transactions().len()).map(|index| {
        (
            COLUMN_BLOCK_BODY,
            packed::TransactionKey::new_builder()
                .block_hash(hash.clone())
                .index(index)
                .build()
                .as_slice()
                .to_vec(),
        )
    }));
    keys
}

/// Commit a new archive with one record per file, using the production writer.
/// Reopening it caches only the latest segment, so historical file loss remains
/// observable instead of being masked by an already-open file descriptor.
pub fn write_segmented_archive(path: &Path, blocks: &[BlockView]) {
    assert!(!path.exists(), "segmented fixture requires a new archive");
    let mut files = FreezerFilesBuilder::new(path.to_path_buf())
        .max_file_size(1)
        .build()
        .unwrap();
    for block in blocks {
        files
            .append(files.number(), block.data().as_slice())
            .unwrap();
    }
    files.sync_all().unwrap();
}

/// Storage faults exercised through actual VM and proof consumers.
#[derive(Clone, Copy, Debug)]
pub enum ArchiveFault {
    /// An uncached historical data file is gone.
    MissingData,
    /// A committed record cannot be read in full.
    TruncatedData,
    /// A committed record no longer matches its checksum.
    CorruptData,
    /// The committed file index cannot be read in full.
    TruncatedIndex,
    /// The DB mapping cannot be decoded.
    MalformedMapping,
    /// The DB mapping names a record outside the committed prefix.
    MissingRecord,
    /// The DB mapping names another block's record.
    WrongBlock,
}

impl ArchiveFault {
    /// The shared consumer fault matrix.
    pub const ALL: [Self; 7] = [
        Self::MissingData,
        Self::TruncatedData,
        Self::CorruptData,
        Self::TruncatedIndex,
        Self::MalformedMapping,
        Self::MissingRecord,
        Self::WrongBlock,
    ];

    /// Damage a freshly reopened segmented fixture without reading its payload.
    /// `block` must precede the last archive record; `other` supplies a wrong hash.
    pub fn inject(self, store: &ChainDB, path: &Path, block: &BlockView, other: &BlockView) {
        let hash = block.hash();
        let mut mapping = store
            .get(COLUMN_BLOCK_ARCHIVE, hash.as_slice())
            .unwrap()
            .to_vec();
        let record = u64::from_le_bytes(mapping[..8].try_into().unwrap());
        assert!(record + 1 < store.freezer().unwrap().number());
        // write_segmented_archive emits record N in file N-1.
        let data = path.join(format!("blk{:06}", record - 1));
        match self {
            Self::MissingData => {
                std::fs::remove_file(data).unwrap();
                return;
            }
            Self::TruncatedData | Self::TruncatedIndex => {
                let path = if matches!(self, Self::TruncatedIndex) {
                    path.join("INDEX")
                } else {
                    data
                };
                let file = OpenOptions::new().write(true).open(path).unwrap();
                file.set_len(0).unwrap();
                file.sync_all().unwrap();
                return;
            }
            Self::CorruptData => {
                let mut file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(data)
                    .unwrap();
                let mut byte = [0];
                file.read_exact(&mut byte).unwrap();
                file.seek(SeekFrom::Start(0)).unwrap();
                file.write_all(&[byte[0] ^ 0x80]).unwrap();
                file.sync_all().unwrap();
                return;
            }
            Self::MalformedMapping => mapping.truncate(1),
            Self::MissingRecord => {
                mapping[..8].copy_from_slice(&store.freezer().unwrap().number().to_le_bytes())
            }
            Self::WrongBlock => {
                assert_eq!(block.number(), other.number());
                assert_ne!(hash, other.hash());
                let other = store
                    .get(COLUMN_BLOCK_ARCHIVE, other.hash().as_slice())
                    .unwrap();
                mapping[..8].copy_from_slice(&other[..8]);
            }
        }
        let mut batch = store.db().new_write_batch();
        batch
            .put(COLUMN_BLOCK_ARCHIVE, hash.as_slice(), &mapping)
            .unwrap();
        store.db().write_sync(&batch).unwrap();
    }

    /// Check the existing fatal storage boundary, not an arbitrary test panic.
    /// Consumers must be invoked without assertions inside the unwind boundary.
    pub fn assert_failure(self, panic: Box<dyn Any + Send>) {
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap();
        let expected = match self {
            Self::MalformedMapping => "valid archive mapping",
            Self::MissingRecord => "committed archive record is missing",
            Self::WrongBlock => "archive mapping hash mismatch",
            _ => "archive",
        };
        assert!(message.contains(expected), "{self:?}: {message}");
        if matches!(
            self,
            Self::MissingData | Self::TruncatedData | Self::CorruptData | Self::TruncatedIndex
        ) {
            assert!(message.contains("read failed"), "{self:?}: {message}");
        }
    }
}
