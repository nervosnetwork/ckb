use crate::{ChainDB, ChainStore};
use ckb_db::{DBIterator, IteratorMode, RocksDB};
use ckb_db_schema::*;
use ckb_freezer::{Freezer, FreezerController, FreezerServiceConfig};
use ckb_types::{
    core::{BlockBuilder, BlockView, EpochNumberWithFraction},
    packed,
    prelude::*,
};

fn block(number: u64) -> BlockView {
    let transactions = (1u32..=3).map(|version| {
        packed::Transaction::new_builder()
            .raw(
                packed::RawTransaction::new_builder()
                    .version(version + u32::try_from(number).unwrap() * 10)
                    .build(),
            )
            .build()
            .into_view()
    });
    BlockBuilder::default()
        .number(number)
        .epoch(EpochNumberWithFraction::new(1, 0, 100))
        .transactions(transactions)
        .extension(Some(vec![0x7f; 96].into()))
        .build()
}

fn assert_body(store: &impl ChainStore, block: &BlockView) {
    let hash = block.hash();
    assert_eq!(store.get_block(&hash).unwrap(), *block);
    assert_eq!(store.get_packed_block(&hash).unwrap(), block.data());
    assert_eq!(store.get_block_body(&hash), block.transactions());
    assert_eq!(store.get_block_txs_hashes(&hash), block.tx_hashes());
    let uncles = store.get_block_uncles(&hash).unwrap();
    assert_eq!(uncles.data().as_slice(), block.uncles().data().as_slice());
    assert_eq!(
        uncles.hashes().as_slice(),
        block.uncles().hashes().as_slice()
    );
    assert_eq!(
        store.get_block_proposal_txs_ids(&hash).unwrap().as_slice(),
        block.data().proposals().as_slice()
    );
    assert_eq!(store.get_block_extension(&hash), block.extension());
    assert_eq!(
        store.get_cellbase(&hash),
        block.transactions().first().cloned()
    );
    for tx in block.transactions() {
        let (read, info) = store.get_transaction_with_info(&tx.hash()).unwrap();
        assert_eq!(read, tx);
        assert_eq!(read.data().as_slice(), tx.data().as_slice());
        assert_eq!(read.hash(), tx.data().calc_tx_hash());
        assert_eq!(read.witness_hash(), tx.data().calc_witness_hash());
        assert_eq!(info.block_hash, hash);
    }
}

#[test]
fn archive_index_rejects_malformed_values_and_mismatched_transaction_counts() {
    use crate::archive::ArchiveIndex;

    let (_directory, store, block) = hot_fixture(23);
    super::db::archive(&store, std::slice::from_ref(&block));
    let raw = store
        .get(COLUMN_BLOCK_ARCHIVE, block.hash().as_slice())
        .unwrap();
    let valid = raw.to_vec();
    assert_eq!(valid.len(), 8 + 32 * block.transactions().len());
    for (raw, tx) in valid[8..].chunks_exact(32).zip(block.transactions()) {
        assert_eq!(raw, tx.witness_hash().as_slice());
    }
    let index = ArchiveIndex::new(raw).unwrap().unwrap();
    let fields = store
        .freezer()
        .unwrap()
        .read_block(index.record(), &block.hash())
        .unwrap()
        .unwrap();
    assert!(index.transaction(&fields, usize::MAX).unwrap().is_none());
    assert_eq!(index.block_view(block.data()).unwrap().data(), block.data());

    let read = |raw: &[u8]| {
        let mut batch = store.db().new_write_batch();
        batch
            .put(COLUMN_BLOCK_ARCHIVE, b"index-fixture", raw)
            .unwrap();
        store.db().write_sync(&batch).unwrap();
        ArchiveIndex::new(store.get(COLUMN_BLOCK_ARCHIVE, b"index-fixture").unwrap())
    };
    assert!(read(&0u64.to_le_bytes()).unwrap().is_none());
    for raw in [
        vec![],
        vec![0; 7],
        vec![1; 9],
        vec![1; 39],
        vec![1; 41],
        vec![0; 40],
    ] {
        assert!(read(&raw).is_err());
    }
    for len in [8, valid.len() - 32, valid.len() + 32] {
        let mut raw = valid.clone();
        raw.resize(len, 0);
        let index = read(&raw).unwrap().unwrap();
        assert!(index.block_view(block.data()).is_err());
        assert!(index.transaction(&fields, 0).is_err());
    }
}

#[test]
fn cached_witness_hashes_preserve_the_original_header_and_survive_collection() {
    let (directory, store, _) = hot_fixture(24);
    let original = block(25);
    let data = original.data();
    let header = data
        .header()
        .as_builder()
        .raw(
            data.header()
                .raw()
                .as_builder()
                .transactions_root(packed::Byte32::default())
                .build(),
        )
        .build();
    let block = data
        .as_builder()
        .header(header)
        .build()
        .into_view_without_reset_header();
    assert_ne!(block.transactions_root(), block.calc_transactions_root());
    let txn = store.begin_transaction();
    txn.insert_block(&block).unwrap();
    txn.attach_block(&block).unwrap();
    txn.commit().unwrap();
    drop(txn);
    super::db::archive(&store, std::slice::from_ref(&block));
    assert_body(&store, &block);
    store.collect_archive(&Default::default(), || {}).unwrap();
    assert_body(&store, &block);
    drop(store);
    let reopened = ChainDB::new_with_freezer(
        RocksDB::open_in(directory.path().join("db"), COLUMNS),
        Freezer::open_in(directory.path().join("archive")).unwrap(),
        Default::default(),
    );
    assert_body(&reopened, &block);
}

fn remove_hot_payload(store: &ChainDB) {
    // An isolated fixture for cold-only reads, before testing the collector itself.
    let mut batch = store.db().new_write_batch();
    for col in [
        COLUMN_BLOCK_BODY,
        COLUMN_BLOCK_UNCLE,
        COLUMN_BLOCK_PROPOSAL_IDS,
        COLUMN_BLOCK_EXTENSION,
        COLUMN_NUMBER_HASH,
    ] {
        let mut iter = store.db().iter(col, IteratorMode::Start).unwrap();
        for (key, _) in iter.by_ref() {
            batch.delete(col, &key).unwrap();
        }
        iter.status().unwrap();
    }
    store.db().write_sync(&batch).unwrap();
}

#[test]
fn every_body_entry_point_reads_the_same_cold_block() {
    for (transaction_count, witness_bytes) in [(3, 0), (3, 16 * 1024), (256, 0)] {
        let directory = tempfile::tempdir().unwrap();
        let store = ChainDB::new_with_freezer(
            RocksDB::open_in(directory.path().join("db"), COLUMNS),
            Freezer::open_in(directory.path().join("archive")).unwrap(),
            Default::default(),
        );
        let original = block(7);
        let block = original
            .as_advanced_builder()
            .set_transactions(
                original
                    .transactions()
                    .into_iter()
                    .cycle()
                    .take(transaction_count)
                    .enumerate()
                    .map(|(index, tx)| {
                        tx.as_advanced_builder()
                            .version(index as u32)
                            .witness(packed::Bytes::from(vec![0x91; witness_bytes]))
                            .build()
                    })
                    .collect(),
            )
            .uncle(block(6).as_uncle())
            .proposal(block(5).transactions()[0].proposal_short_id())
            .build();
        let transaction = store.begin_transaction();
        transaction.insert_block(&block).unwrap();
        transaction.attach_block(&block).unwrap();
        transaction.commit().unwrap();
        drop(transaction);
        let before = store.get_snapshot();
        assert_body(&before, &block);
        super::db::archive(&store, std::slice::from_ref(&block));
        remove_hot_payload(&store);
        assert_eq!(
            store
                .db()
                .iter(COLUMN_BLOCK_BODY, IteratorMode::Start)
                .unwrap()
                .count(),
            0
        );
        assert_body(&store, &block);
        assert_body(&store.get_snapshot(), &block);
        assert_body(&before, &block);
        let txn = store.begin_transaction();
        assert_body(&txn, &block);
        assert_body(&txn.get_snapshot(), &block);
        drop((txn, before, store));
        let store = ChainDB::new_with_freezer(
            RocksDB::open_in(directory.path().join("db"), COLUMNS),
            Freezer::open_in(directory.path().join("archive")).unwrap(),
            Default::default(),
        );
        assert_eq!(store.recover_archive().unwrap(), 2);
        assert_body(&store.get_snapshot(), &block);
    }
}

#[test]
fn cold_payload_mutations_and_savepoint_rollback_use_transaction_state() {
    let directory = tempfile::tempdir().unwrap();
    let store = ChainDB::new_with_freezer(
        RocksDB::open_in(directory.path().join("db"), COLUMNS),
        Freezer::open_in(directory.path().join("archive")).unwrap(),
        Default::default(),
    );
    let block = block(4);
    let txn = store.begin_transaction();
    txn.insert_block(&block).unwrap();
    txn.attach_block(&block).unwrap();
    txn.commit().unwrap();
    drop(txn);
    super::db::archive(&store, std::slice::from_ref(&block));
    remove_hot_payload(&store);
    assert_body(&store, &block);
    let txn = store.begin_transaction();
    txn.inner.set_savepoint();
    let key = packed::TransactionKey::new_builder()
        .block_hash(block.hash())
        .index(0usize)
        .build();
    txn.delete(COLUMN_BLOCK_BODY, key.as_slice()).unwrap();
    assert_eq!(txn.get_block_body(&block.hash()), block.transactions()[1..]);
    assert_eq!(
        txn.get_block_txs_hashes(&block.hash()),
        block.tx_hashes()[1..]
    );
    assert!(txn.get_cellbase(&block.hash()).is_none());
    assert!(
        txn.get_transaction(&block.transactions()[0].hash())
            .is_none()
    );
    assert_eq!(
        txn.get_snapshot().get_block_body(&block.hash()),
        block.transactions()[1..]
    );
    assert_body(&store, &block);
    txn.inner.rollback_to_savepoint().unwrap();
    assert_body(&txn, &block);
    txn.delete(COLUMN_BLOCK_EXTENSION, block.hash().as_slice())
        .unwrap();
    assert!(txn.get_block_extension(&block.hash()).is_none());
    assert_eq!(txn.get_block_body(&block.hash()), block.transactions());
    txn.inner.rollback().unwrap();
    assert_body(&txn, &block);
    drop(txn);
    // A batch can be cleared and reused without suppressing a later restoration.
    let mut batch = store.new_write_batch();
    batch.delete(COLUMN_BLOCK_BODY, key.as_slice()).unwrap();
    batch.clear().unwrap();
    batch
        .delete(COLUMN_BLOCK_EXTENSION, block.hash().as_slice())
        .unwrap();
    store.write_sync(&batch).unwrap();
    assert_eq!(
        store
            .get(COLUMN_BLOCK_ARCHIVE, block.hash().as_slice())
            .unwrap()
            .as_ref(),
        0u64.to_le_bytes()
    );
    assert!(
        store
            .get(COLUMN_BLOCK_EXTENSION, block.hash().as_slice())
            .is_none()
    );
    assert_eq!(
        store.get_hot_block_body(&block.hash()),
        block.transactions()
    );
}

#[test]
fn orphan_records_recover_and_missing_committed_archive_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let store = ChainDB::new_with_freezer(
        RocksDB::open_in(directory.path().join("db"), COLUMNS),
        Freezer::open_in(directory.path().join("archive")).unwrap(),
        Default::default(),
    );
    let block = block(19);
    let txn = store.begin_transaction();
    txn.insert_block(&block).unwrap();
    txn.attach_block(&block).unwrap();
    txn.insert_block_ext(
        &block.hash(),
        &ckb_types::core::BlockExt {
            received_at: 0,
            total_difficulty: Default::default(),
            total_uncles_count: 0,
            verified: Some(true),
            txs_fees: vec![],
            cycles: None,
            txs_sizes: None,
        },
    )
    .unwrap();
    txn.commit().unwrap();
    drop(txn);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let controller = FreezerController::start(
        store.freezer().unwrap().clone(),
        runtime.handle().clone(),
        FreezerServiceConfig::default(),
    )
    .unwrap();
    runtime
        .block_on(controller.append(vec![block.data(), block.data()]))
        .unwrap();
    runtime.block_on(controller.shutdown()).unwrap();
    assert!(
        store
            .get(COLUMN_BLOCK_ARCHIVE, block.hash().as_slice())
            .is_none()
    );
    drop((controller, store));
    let store = ChainDB::new_with_freezer(
        RocksDB::open_in(directory.path().join("db"), COLUMNS),
        Freezer::open_in(directory.path().join("archive")).unwrap(),
        Default::default(),
    );
    assert_eq!(store.recover_archive().unwrap(), 3);
    assert_eq!(store.recover_archive().unwrap(), 3);
    assert_body(&store, &block);
    let db = store.into_inner();
    let missing = ChainDB::new(db, Default::default());
    assert!(missing.recover_archive().is_err());
}

#[test]
fn a_never_archived_database_gets_the_new_empty_index_column() {
    let directory = tempfile::tempdir().unwrap();
    drop(RocksDB::open_in(directory.path(), COLUMNS - 1));
    let db = RocksDB::open_in(directory.path(), COLUMNS);
    assert!(
        db.get_pinned(COLUMN_BLOCK_ARCHIVE, b"missing")
            .unwrap()
            .is_none()
    );
}

#[test]
#[cfg(feature = "legacy-db-test")]
#[ignore = "requires CKB_LEGACY_WRITER and a new CKB_UPGRADE_DIR"]
fn old_engine_hot_data_survives_upgrade_archival_and_backup() {
    use std::{fs, io::Write, path::Path, process::Command};

    fn copy_directory(from: &Path, to: &Path) {
        fs::create_dir(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_directory(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    let writer = std::env::var_os("CKB_LEGACY_WRITER").expect("old engine writer binary");
    let root = std::path::PathBuf::from(
        std::env::var_os("CKB_UPGRADE_DIR").expect("new upgrade evidence directory"),
    );
    fs::create_dir(&root).unwrap();
    let (_source_directory, source, block) = hot_fixture(421);
    let seed = root.join("hot-rows");
    let mut output = fs::File::create(&seed).unwrap();
    // The pre-Freezer hot schema contains columns 0 through 18.
    for column in [
        "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16",
        "17", "18",
    ] {
        let mut rows = source.db().iter(column, IteratorMode::Start).unwrap();
        for (key, value) in rows.by_ref() {
            output
                .write_all(&column.parse::<u32>().unwrap().to_le_bytes())
                .unwrap();
            output.write_all(&(key.len() as u32).to_le_bytes()).unwrap();
            output
                .write_all(&(value.len() as u32).to_le_bytes())
                .unwrap();
            output.write_all(&key).unwrap();
            output.write_all(&value).unwrap();
        }
        rows.status().unwrap();
    }
    drop(output);
    for mode in ["sst", "wal"] {
        let directory = root.join(mode);
        fs::create_dir(&directory).unwrap();
        let path = directory.join("db");
        let status = Command::new(&writer)
            .arg(&seed)
            .arg(&path)
            .arg(mode)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(fs::read_dir(&path).unwrap().any(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|extension| extension == "sst")
        }));
        copy_directory(&path, &directory.join("legacy"));

        let upgraded = ChainDB::new(RocksDB::open_in(&path, COLUMNS), Default::default());
        assert_body(&upgraded, &block);
        drop(upgraded);
        let archive_path = directory.join("archive");
        let store = ChainDB::new_with_freezer(
            RocksDB::open_in(&path, COLUMNS),
            Freezer::open_in(&archive_path).unwrap(),
            Default::default(),
        );
        super::db::archive(&store, std::slice::from_ref(&block));
        assert!(
            store
                .collect_archive(&Default::default(), || {})
                .unwrap()
                .archived
                > 0
        );
        assert_body(&store, &block);
        let key = packed::TransactionKey::new_builder()
            .block_hash(block.hash())
            .index(0usize)
            .build();
        assert!(
            store
                .db()
                .get_pinned(COLUMN_BLOCK_BODY, key.as_slice())
                .unwrap()
                .is_none()
        );
        drop(store);

        let backup = directory.join("backup");
        fs::create_dir(&backup).unwrap();
        copy_directory(&path, &backup.join("db"));
        copy_directory(&archive_path, &backup.join("archive"));
        for location in [&directory, &backup] {
            let reopened = ChainDB::new_with_freezer(
                RocksDB::open_in(location.join("db"), COLUMNS),
                Freezer::open_in(location.join("archive")).unwrap(),
                Default::default(),
            );
            reopened.recover_archive().unwrap();
            assert_body(&reopened, &block);
        }
    }
}

fn hot_fixture(number: u64) -> (tempfile::TempDir, ChainDB, BlockView) {
    let directory = tempfile::tempdir().unwrap();
    let store = ChainDB::new_with_freezer(
        RocksDB::open_in(directory.path().join("db"), COLUMNS),
        Freezer::open_in(directory.path().join("archive")).unwrap(),
        Default::default(),
    );
    let block = block(number);
    let txn = store.begin_transaction();
    txn.insert_block(&block).unwrap();
    txn.attach_block(&block).unwrap();
    txn.insert_block_ext(
        &block.hash(),
        &ckb_types::core::BlockExt {
            received_at: 0,
            total_difficulty: Default::default(),
            total_uncles_count: 0,
            verified: Some(true),
            txs_fees: vec![],
            cycles: None,
            txs_sizes: None,
        },
    )
    .unwrap();
    txn.commit().unwrap();
    drop(txn);
    (directory, store, block)
}

fn append_without_index(store: &ChainDB, block: &BlockView) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let controller = FreezerController::start(
        store.freezer().unwrap().clone(),
        runtime.handle().clone(),
        FreezerServiceConfig::default(),
    )
    .unwrap();
    runtime
        .block_on(controller.append(vec![block.data()]))
        .unwrap();
    runtime.block_on(controller.shutdown()).unwrap();
}

#[test]
fn separately_constructed_batches_preserve_both_cold_deletions() {
    let (_directory, store, block) = hot_fixture(8);
    super::db::archive(&store, std::slice::from_ref(&block));
    remove_hot_payload(&store);
    let before = store.get_snapshot();
    let mut first = store.new_write_batch();
    let mut second = store.new_write_batch();
    for (index, batch) in [&mut first, &mut second].into_iter().enumerate() {
        let key = packed::TransactionKey::new_builder()
            .block_hash(block.hash())
            .index(index)
            .build();
        batch.delete(COLUMN_BLOCK_BODY, key.as_slice()).unwrap();
    }
    store.write_sync(&first).unwrap();
    store.write_sync(&second).unwrap();
    assert_eq!(
        store.get_block_body(&block.hash()),
        block.transactions()[2..]
    );
    assert_eq!(
        store.get_block_txs_hashes(&block.hash()),
        block.tx_hashes()[2..]
    );
    assert!(store.get_cellbase(&block.hash()).is_none());
    assert_body(&before, &block);
    // Reusing a submitted batch must not restore either deleted transaction.
    store.write_sync(&second).unwrap();
    assert_eq!(
        store.get_block_body(&block.hash()),
        block.transactions()[2..]
    );
}

#[test]
fn competing_cold_transactions_detect_a_conflicting_restoration() {
    let (_directory, store, block) = hot_fixture(9);
    super::db::archive(&store, std::slice::from_ref(&block));
    remove_hot_payload(&store);
    let first = store.begin_transaction();
    let second = store.begin_transaction();
    for (index, txn) in [&first, &second].into_iter().enumerate() {
        let key = packed::TransactionKey::new_builder()
            .block_hash(block.hash())
            .index(index)
            .build();
        txn.delete(COLUMN_BLOCK_BODY, key.as_slice()).unwrap();
    }
    first.commit().unwrap();
    assert!(second.commit().is_err());
    drop((first, second));
    let retry = store.begin_transaction();
    let key = packed::TransactionKey::new_builder()
        .block_hash(block.hash())
        .index(1usize)
        .build();
    retry.delete(COLUMN_BLOCK_BODY, key.as_slice()).unwrap();
    retry.commit().unwrap();
    assert_eq!(
        store.get_block_body(&block.hash()),
        block.transactions()[2..]
    );
}

#[test]
fn recovery_waits_for_staged_writers_and_does_not_publish_stale_files() {
    let (_directory, store, block) = hot_fixture(10);
    append_without_index(&store, &block);
    let txn = store.begin_transaction();
    let key = packed::TransactionKey::new_builder()
        .block_hash(block.hash())
        .index(0usize)
        .build();
    txn.delete(COLUMN_BLOCK_BODY, key.as_slice()).unwrap();
    assert_eq!(store.recover_archive().unwrap(), 1);
    assert!(
        store
            .get(COLUMN_BLOCK_ARCHIVE, block.hash().as_slice())
            .is_none()
    );
    txn.commit().unwrap();
    drop(txn);
    assert_eq!(store.recover_archive().unwrap(), 2);
    assert!(store.get_archived_block(&block.hash()).is_none());
    assert_eq!(
        store.get_block_body(&block.hash()),
        block.transactions()[1..]
    );
}

#[test]
fn deleted_header_cache_and_hot_override_cannot_republish_an_orphan() {
    let (_directory, store, block) = hot_fixture(11);
    append_without_index(&store, &block);
    // Prime the shared cache before deletion; recovery must inspect the DB view.
    store.get_block_header(&block.hash()).unwrap();
    let txn = store.begin_transaction();
    txn.delete_block(&block).unwrap();
    txn.commit().unwrap();
    drop(txn);
    assert_eq!(store.recover_archive().unwrap(), 2);
    assert!(store.get_archived_block(&block.hash()).is_none());
    assert!(store.get_block(&block.hash()).is_none());

    let (_directory, store, block) = hot_fixture(12);
    super::db::archive(&store, std::slice::from_ref(&block));
    append_without_index(&store, &block);
    remove_hot_payload(&store);
    let txn = store.begin_transaction();
    txn.delete(COLUMN_BLOCK_EXTENSION, block.hash().as_slice())
        .unwrap();
    txn.commit().unwrap();
    drop(txn);
    assert_eq!(store.recover_archive().unwrap(), 3);
    assert!(store.get_archived_block(&block.hash()).is_none());
    assert!(store.get_block_extension(&block.hash()).is_none());
}

#[test]
fn whole_body_and_block_batch_deletions_invalidate_cold_reads() {
    for delete_header in [false, true] {
        let (_directory, store, block) = hot_fixture(13);
        super::db::archive(&store, std::slice::from_ref(&block));
        remove_hot_payload(&store);
        let mut batch = store.new_write_batch();
        if delete_header {
            batch
                .delete_block(
                    block.number(),
                    &block.hash(),
                    block.transactions().len() as u32,
                )
                .unwrap();
        } else {
            batch
                .delete_block_body(
                    block.number(),
                    &block.hash(),
                    block.transactions().len() as u32,
                )
                .unwrap();
        }
        store.write_sync(&batch).unwrap();
        assert!(store.get_archived_block(&block.hash()).is_none());
        assert!(store.get_block_body(&block.hash()).is_empty());
        assert!(store.get_cellbase(&block.hash()).is_none());
        assert!(store.get_block_uncles(&block.hash()).is_none());
        if delete_header {
            assert!(store.get_block(&block.hash()).is_none());
        }
    }
}

#[test]
fn physical_collection_keeps_all_read_views_and_hot_overrides_consistent() {
    let (directory, store, cold) = hot_fixture(21);
    let hot = block(22);
    let restored = block(23);
    let txn = store.begin_transaction();
    for block in [&hot, &restored] {
        txn.insert_block(block).unwrap();
        txn.attach_block(block).unwrap();
    }
    txn.commit().unwrap();
    drop(txn);
    let before_archive = store.get_snapshot();
    super::db::archive(&store, &[cold.clone(), restored.clone()]);
    let before_collection = store.get_snapshot();
    let txn = store.begin_transaction();
    txn.delete(COLUMN_BLOCK_EXTENSION, restored.hash().as_slice())
        .unwrap();
    txn.commit().unwrap();
    drop(txn);
    let stats = store
        .collect_archive(&ckb_db::CollectionOptions::default(), || {})
        .unwrap();
    assert_eq!(stats.generation, 1);
    assert_eq!(stats.archived, 7); // three transactions plus four payload columns
    assert_eq!(stats.archive_frontier, 3);
    assert!(!store.archive_collection_due().unwrap());
    for view in [&before_archive, &before_collection] {
        assert_body(view, &cold);
        assert_body(view, &restored);
    }
    assert_body(&store, &cold);
    assert_body(&store, &hot);
    assert!(store.get_block_extension(&restored.hash()).is_none());
    assert_eq!(
        store.get_block_body(&restored.hash()),
        restored.transactions()
    );
    assert!(
        store
            .db()
            .get_pinned(
                COLUMN_BLOCK_BODY,
                packed::TransactionKey::new_builder()
                    .block_hash(cold.hash())
                    .index(0usize)
                    .build()
                    .as_slice()
            )
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .db()
            .get_pinned(
                COLUMN_BLOCK_BODY,
                packed::TransactionKey::new_builder()
                    .block_hash(restored.hash())
                    .index(0usize)
                    .build()
                    .as_slice()
            )
            .unwrap()
            .is_some()
    );
    drop((before_archive, before_collection, store));
    let store = ChainDB::new_with_freezer(
        RocksDB::open_in(directory.path().join("db"), COLUMNS),
        Freezer::open_in(directory.path().join("archive")).unwrap(),
        Default::default(),
    );
    store.recover_archive().unwrap();
    assert_body(&store, &cold);
    assert_body(&store, &hot);
    assert_eq!(
        store.get_block_body(&restored.hash()),
        restored.transactions()
    );
    assert!(store.get_block_extension(&restored.hash()).is_none());
}

#[test]
fn corrupt_archive_aborts_collection_before_the_hot_backup_is_removed() {
    use std::io::{Read, Seek, SeekFrom, Write};
    let (directory, store, block) = hot_fixture(31);
    super::db::archive(&store, std::slice::from_ref(&block));
    let path = directory.path().join("archive/blk000000");
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let mut byte = [0];
    file.read_exact(&mut byte).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write_all(&[byte[0] ^ 0x80]).unwrap();
    file.sync_all().unwrap();
    assert!(
        store
            .collect_archive(&ckb_db::CollectionOptions::default(), || panic!(
                "corrupt source must not publish"
            ))
            .is_err()
    );
    assert_eq!(
        store.get_hot_block_body(&block.hash()),
        block.transactions()
    );
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write_all(&byte).unwrap();
    file.sync_all().unwrap();
    store
        .collect_archive(&ckb_db::CollectionOptions::default(), || {})
        .unwrap();
    assert_body(&store, &block);
}
