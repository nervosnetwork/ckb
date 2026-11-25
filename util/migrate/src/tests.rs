use crate::migrate::Migrate;
use crate::sst_rebuild::SstRebuild;
use ckb_app_config::DBConfig;
use ckb_chain_spec::consensus::build_genesis_epoch_ext;
use ckb_db::{DBIterator, IteratorMode, RocksDB, Schema};
use ckb_db_schema::{
    BlockHashIndexValue, BlockIndexFlag, COLUMN_BLOCK_BODY, COLUMN_BLOCK_EXT, COLUMN_BLOCK_HEADER,
    COLUMN_HASH_INDEX, COLUMN_INDEX, META_CURRENT_EPOCH_KEY, META_TIP_HEADER_KEY,
    MIGRATION_VERSION_KEY, block_body_key, block_key, block_number_key, legacy,
};
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{
    core::{
        BlockBuilder, BlockExt, Capacity, EpochNumberWithFraction, HeaderBuilder,
        TransactionBuilder, capacity_bytes, hardfork::HardForks,
    },
    packed::{self, Bytes},
    prelude::*,
    utilities::DIFF_TWO,
};

#[test]
fn test_mock_migration_requires_sst_rebuild() {
    let tmp_dir = tempfile::Builder::new()
        .prefix("test_mock_migration")
        .tempdir()
        .unwrap();
    let config = DBConfig {
        path: tmp_dir.as_ref().to_path_buf(),
        ..Default::default()
    };
    // 0.25-0.34 ckb's columns is 12
    let db = RocksDB::open_with_columns(&config, 12);
    let cellbase = TransactionBuilder::default()
        .witness(Bytes::default())
        .build();
    let epoch_ext =
        build_genesis_epoch_ext(capacity_bytes!(100), DIFF_TWO, 1_000, 4 * 60 * 60, (1, 40));
    let genesis = BlockBuilder::default().transaction(cellbase).build();

    // genesis block insert is copy from 0.34 ckb
    let db_txn = db.transaction();

    // insert block
    {
        let hash = genesis.hash();
        let header: packed::HeaderView = genesis.header().into();
        let number = header.data().raw().number();
        let uncles: packed::UncleBlockVecView = genesis.uncles().into();
        let proposals = genesis.data().proposals();
        db_txn
            .put(legacy::COLUMN_INDEX, number.as_slice(), hash.as_slice())
            .unwrap();
        db_txn
            .put(
                legacy::COLUMN_BLOCK_HEADER,
                hash.as_slice(),
                header.as_slice(),
            )
            .unwrap();
        db_txn
            .put(
                legacy::COLUMN_BLOCK_UNCLE,
                hash.as_slice(),
                uncles.as_slice(),
            )
            .unwrap();
        db_txn
            .put(
                legacy::COLUMN_BLOCK_PROPOSAL_IDS,
                hash.as_slice(),
                proposals.as_slice(),
            )
            .unwrap();
        for (index, tx) in genesis.transactions().into_iter().enumerate() {
            let key = packed::TransactionKey::new_builder()
                .block_hash(hash.clone())
                .index(index)
                .build();
            let tx_data = Into::<packed::TransactionView>::into(tx);
            db_txn
                .put(
                    legacy::COLUMN_BLOCK_BODY,
                    key.as_slice(),
                    tx_data.as_slice(),
                )
                .unwrap();
        }
    }

    let ext = BlockExt {
        received_at: unix_time_as_millis(),
        total_difficulty: genesis.difficulty(),
        total_uncles_count: 0,
        verified: None,
        txs_fees: vec![],
        cycles: None,
        txs_sizes: None,
    };

    // insert_block_epoch_index
    {
        db_txn
            .put(
                legacy::COLUMN_BLOCK_EPOCH,
                genesis.header().hash().as_slice(),
                epoch_ext.last_block_hash_in_previous_epoch().as_slice(),
            )
            .unwrap()
    }
    // insert epoch ext
    {
        db_txn
            .put(
                legacy::COLUMN_EPOCH,
                epoch_ext.last_block_hash_in_previous_epoch().as_slice(),
                Into::<packed::EpochExt>::into(&epoch_ext).as_slice(),
            )
            .unwrap();
        let epoch_number: packed::Uint64 = epoch_ext.number().into();
        db_txn
            .put(
                legacy::COLUMN_EPOCH,
                epoch_number.as_slice(),
                epoch_ext.last_block_hash_in_previous_epoch().as_slice(),
            )
            .unwrap()
    }

    // insert tip header
    {
        db_txn
            .put(
                legacy::COLUMN_META,
                META_TIP_HEADER_KEY,
                genesis.header().hash().as_slice(),
            )
            .unwrap()
    }

    // insert block ext
    {
        db_txn
            .put(
                legacy::COLUMN_BLOCK_EXT,
                genesis.header().hash().as_slice(),
                Into::<packed::BlockExtV1>::into(ext).as_slice(),
            )
            .unwrap()
    }

    // insert_current_epoch_ext
    {
        db_txn
            .put(
                legacy::COLUMN_META,
                META_CURRENT_EPOCH_KEY,
                Into::<packed::EpochExt>::into(epoch_ext).as_slice(),
            )
            .unwrap()
    }

    db_txn.commit().unwrap();

    drop(db_txn);
    drop(db);

    let mg = Migrate::new(tmp_dir.as_ref().to_path_buf(), HardForks::new_mirana());

    let db = mg.open_bulk_load_db().unwrap().unwrap();

    assert!(mg.migrate(db, false).is_err());
}

#[test]
fn test_sst_rebuild_migrates_old_block_keys() {
    let tmp_dir = tempfile::Builder::new()
        .prefix("test_sst_rebuild_migrates_old_block_keys")
        .tempdir()
        .unwrap();
    let db_path = tmp_dir.as_ref().join("db");
    let config = DBConfig {
        path: db_path.clone(),
        ..Default::default()
    };
    let db = RocksDB::open(&config, Schema::Legacy);

    let genesis = BlockBuilder::default()
        .transaction(
            TransactionBuilder::default()
                .witness(Bytes::default())
                .build(),
        )
        .build();
    let block1 = BlockBuilder::default()
        .header(
            HeaderBuilder::default()
                .parent_hash(genesis.hash())
                .number(1)
                .epoch(EpochNumberWithFraction::new(0, 1, 10))
                .nonce(1)
                .build(),
        )
        .transaction(
            TransactionBuilder::default()
                .witness(Bytes::from(vec![1]))
                .build(),
        )
        .transaction(
            TransactionBuilder::default()
                .witness(Bytes::from(vec![2]))
                .build(),
        )
        .build_unchecked();
    let fork1 = BlockBuilder::default()
        .header(
            HeaderBuilder::default()
                .parent_hash(genesis.hash())
                .number(1)
                .epoch(EpochNumberWithFraction::new(0, 1, 10))
                .nonce(2)
                .build(),
        )
        .transaction(
            TransactionBuilder::default()
                .witness(Bytes::from(vec![3]))
                .build(),
        )
        .build_unchecked();

    let db_txn = db.transaction();
    insert_old_layout_block(&db_txn, &genesis);
    insert_old_layout_block(&db_txn, &block1);
    insert_old_layout_block(&db_txn, &fork1);

    let orphan_hash = packed::Byte32::new([42; 32]);
    let orphan_ext = BlockExt {
        received_at: unix_time_as_millis(),
        total_difficulty: block1.difficulty(),
        total_uncles_count: 0,
        verified: Some(false),
        txs_fees: vec![],
        cycles: None,
        txs_sizes: None,
    };
    let orphan_tx_key = packed::TransactionKey::new_builder()
        .block_hash(orphan_hash.clone())
        .index(0u32)
        .build();
    let orphan_tx = Into::<packed::TransactionView>::into(TransactionBuilder::default().build());
    db_txn
        .put(
            legacy::COLUMN_BLOCK_EXT,
            orphan_hash.as_slice(),
            Into::<packed::BlockExtV1>::into(orphan_ext).as_slice(),
        )
        .unwrap();
    db_txn
        .put(
            legacy::COLUMN_BLOCK_EPOCH,
            orphan_hash.as_slice(),
            block1.hash().as_slice(),
        )
        .unwrap();
    db_txn
        .put(
            legacy::COLUMN_BLOCK_FILTER,
            orphan_hash.as_slice(),
            b"filter",
        )
        .unwrap();
    db_txn
        .put(
            legacy::COLUMN_BLOCK_FILTER_HASH,
            orphan_hash.as_slice(),
            b"filter_hash",
        )
        .unwrap();
    let orphan_tx_key_1 = packed::TransactionKey::new_builder()
        .block_hash(orphan_hash.clone())
        .index(1u32)
        .build();
    db_txn
        .put(
            legacy::COLUMN_BLOCK_BODY,
            orphan_tx_key.as_slice(),
            orphan_tx.as_slice(),
        )
        .unwrap();
    db_txn
        .put(
            legacy::COLUMN_BLOCK_BODY,
            orphan_tx_key_1.as_slice(),
            orphan_tx.as_slice(),
        )
        .unwrap();

    for block in [&genesis, &block1] {
        let number: packed::Uint64 = block.number().into();
        db_txn
            .put(
                legacy::COLUMN_INDEX,
                number.as_slice(),
                block.hash().as_slice(),
            )
            .unwrap();
    }
    db_txn
        .put(
            legacy::COLUMN_META,
            META_TIP_HEADER_KEY,
            block1.hash().as_slice(),
        )
        .unwrap();
    db_txn.commit().unwrap();
    drop(db_txn);
    db.put_default(MIGRATION_VERSION_KEY, b"20231101000000")
        .unwrap();
    drop(db);

    SstRebuild::new(db_path.clone()).unwrap().run().unwrap();
    let rerun_error = SstRebuild::new(db_path.clone()).unwrap().run().unwrap_err();
    assert!(
        rerun_error
            .to_string()
            .contains("database already has the SST rebuild migration version"),
        "{rerun_error}"
    );

    let migrated = RocksDB::open_in(&db_path, Schema::V1);
    let block1_key = block_key(1, &block1.hash());
    assert!(
        migrated
            .get_pinned(COLUMN_BLOCK_HEADER, &block1_key)
            .unwrap()
            .is_some()
    );
    assert!(
        migrated
            .get_pinned(COLUMN_BLOCK_HEADER, block1.hash().as_slice())
            .unwrap()
            .is_none()
    );

    let fork1_key = block_key(1, &fork1.hash());
    assert!(
        migrated
            .get_pinned(COLUMN_BLOCK_HEADER, &fork1_key)
            .unwrap()
            .is_some()
    );

    let tx_key = block_body_key(1, &block1.hash(), 1);
    assert!(
        migrated
            .get_pinned(COLUMN_BLOCK_BODY, &tx_key)
            .unwrap()
            .is_some()
    );
    assert!(
        migrated
            .get_pinned(COLUMN_BLOCK_BODY, orphan_tx_key.as_slice())
            .unwrap()
            .is_none()
    );
    assert!(
        migrated
            .get_pinned(COLUMN_BLOCK_BODY, orphan_tx_key_1.as_slice())
            .unwrap()
            .is_none()
    );

    let mut expected_body = Vec::new();
    for (number, block) in [(0u64, &genesis), (1u64, &block1), (1u64, &fork1)] {
        for (index, tx) in block.transactions().into_iter().enumerate() {
            let key = block_body_key(number, &block.hash(), index as u32).to_vec();
            let value = Into::<packed::TransactionView>::into(tx)
                .as_slice()
                .to_vec();
            expected_body.push((key, value));
        }
    }
    expected_body.sort_by(|left, right| left.0.cmp(&right.0));

    let mut actual_body = Vec::new();
    for (key, value) in migrated
        .iter(COLUMN_BLOCK_BODY, IteratorMode::Start)
        .unwrap()
    {
        assert_eq!(key.len(), 44);
        actual_body.push((key.to_vec(), value.to_vec()));
    }
    assert_eq!(actual_body, expected_body);
    assert_eq!(actual_body.len(), 4);

    let extra_tx_key = block_body_key(1, &block1.hash(), 2);
    assert!(
        migrated
            .get_pinned(COLUMN_BLOCK_BODY, &extra_tx_key)
            .unwrap()
            .is_none()
    );
    assert!(
        migrated
            .get_pinned(COLUMN_BLOCK_EXT, orphan_hash.as_slice())
            .unwrap()
            .is_none()
    );
    assert!(
        migrated
            .get_pinned(COLUMN_HASH_INDEX, orphan_hash.as_slice())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        migrated
            .get_pinned(COLUMN_INDEX, &block_number_key(1))
            .unwrap()
            .as_deref(),
        Some(block1.hash().as_slice())
    );
    let old_little_endian_key: packed::Uint64 = 1u64.into();
    assert!(
        migrated
            .get_pinned(COLUMN_INDEX, old_little_endian_key.as_slice())
            .unwrap()
            .is_none()
    );

    let canonical_index = migrated
        .get_pinned(COLUMN_HASH_INDEX, block1.hash().as_slice())
        .unwrap()
        .unwrap();
    assert_eq!(
        BlockHashIndexValue::decode(canonical_index.as_ref()),
        Some(BlockHashIndexValue {
            number: 1,
            flag: BlockIndexFlag::MainChain,
        })
    );

    let fork_index = migrated
        .get_pinned(COLUMN_HASH_INDEX, fork1.hash().as_slice())
        .unwrap()
        .unwrap();
    assert_eq!(
        BlockHashIndexValue::decode(fork_index.as_ref()),
        Some(BlockHashIndexValue {
            number: 1,
            flag: BlockIndexFlag::Fork,
        })
    );
}

#[test]
fn test_sst_rebuild_rejects_concurrent_lock() {
    let tmp_dir = tempfile::Builder::new()
        .prefix("test_sst_rebuild_rejects_concurrent_lock")
        .tempdir()
        .unwrap();
    let db_path = tmp_dir.as_ref().join("db");
    let db = create_minimal_legacy_db(&db_path);

    // Keep primary legacy DB handle open to simulate a live `ckb run` process.
    let err = SstRebuild::new(db_path.clone()).unwrap().run().unwrap_err();
    assert!(
        err.to_string().contains("failed to open the database"),
        "{err}"
    );
    assert!(!Migrate::has_pending_sst_rebuild(&db_path));
    assert!(!tmp_dir.as_ref().join("db.migrating").exists());
    drop(db);
}

#[test]
fn test_sst_rebuild_rejects_tip_mismatch_and_cleans_staging() {
    let tmp_dir = tempfile::Builder::new()
        .prefix("test_sst_rebuild_rejects_tip_mismatch")
        .tempdir()
        .unwrap();
    let db_path = tmp_dir.as_ref().join("db");
    let db = create_minimal_legacy_db(&db_path);

    // Corrupt COLUMN_INDEX at height 0 so it disagrees with META_TIP_HEADER_KEY.
    let txn = db.transaction();
    let zero_key: packed::Uint64 = 0u64.into();
    let wrong_hash = packed::Byte32::new([9u8; 32]);
    txn.put(
        legacy::COLUMN_INDEX,
        zero_key.as_slice(),
        wrong_hash.as_slice(),
    )
    .unwrap();
    txn.commit().unwrap();
    drop(txn);
    drop(db);

    let err = SstRebuild::new(db_path.clone()).unwrap().run().unwrap_err();
    assert!(
        err.to_string()
            .contains("does not match META_TIP_HEADER_KEY"),
        "{err}"
    );
    assert!(!tmp_dir.as_ref().join("db.migrating").exists());
    assert!(!Migrate::has_pending_sst_rebuild(&db_path));
    assert_eq!(
        RocksDB::detect_schema(&db_path).unwrap(),
        Some(Schema::Legacy)
    );
}

#[test]
fn test_sst_rebuild_recovers_interrupted_cutover_states() {
    // Case A: crash between `rename(old -> backup)` and `rename(migrating -> old)`,
    // followed by `app_config` recreating an empty `db` directory.
    {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_sst_rebuild_recover_mid_swap")
            .tempdir()
            .unwrap();
        let db_path = tmp_dir.as_ref().join("db");
        drop(create_minimal_legacy_db(&db_path));
        SstRebuild::new(db_path.clone()).unwrap().run().unwrap();

        let backup_path = std::fs::read_dir(tmp_dir.as_ref())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("db.pre-sst-rebuild-")
            })
            .unwrap();
        let migrating_path = tmp_dir.as_ref().join("db.migrating");
        std::fs::rename(&db_path, &migrating_path).unwrap();
        std::fs::create_dir(&db_path).unwrap();

        let marker_path = tmp_dir.as_ref().join("db.sst-rebuild-promoting");
        std::fs::write(
            &marker_path,
            format!(
                "db.migrating\n{}\n",
                backup_path.file_name().unwrap().to_string_lossy()
            ),
        )
        .unwrap();

        assert!(Migrate::has_pending_sst_rebuild(&db_path));
        let mut post_swap_called = false;
        SstRebuild::new(db_path.clone())
            .unwrap()
            .run_with_post_swap(|| {
                post_swap_called = true;
                Ok(())
            })
            .unwrap();

        assert!(post_swap_called);
        assert!(!Migrate::has_pending_sst_rebuild(&db_path));
        assert_eq!(RocksDB::detect_schema(&db_path).unwrap(), Some(Schema::V1));
        assert_eq!(
            RocksDB::detect_schema(&backup_path).unwrap(),
            Some(Schema::Legacy)
        );
    }

    // Case B: crash after atomic directory exchange (`db` is V1, `db.migrating` is Legacy),
    // before moving swapped-out legacy DB to backup and running post_swap.
    {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_sst_rebuild_recover_post_exchange")
            .tempdir()
            .unwrap();
        let db_path = tmp_dir.as_ref().join("db");
        drop(create_minimal_legacy_db(&db_path));
        SstRebuild::new(db_path.clone()).unwrap().run().unwrap();

        let backup_path = std::fs::read_dir(tmp_dir.as_ref())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("db.pre-sst-rebuild-")
            })
            .unwrap();
        let migrating_path = tmp_dir.as_ref().join("db.migrating");
        std::fs::rename(&backup_path, &migrating_path).unwrap();

        let marker_path = tmp_dir.as_ref().join("db.sst-rebuild-promoting");
        std::fs::write(
            &marker_path,
            format!(
                "db.migrating\n{}\n",
                backup_path.file_name().unwrap().to_string_lossy()
            ),
        )
        .unwrap();

        let mut post_swap_called = false;
        SstRebuild::new(db_path.clone())
            .unwrap()
            .run_with_post_swap(|| {
                post_swap_called = true;
                Ok(())
            })
            .unwrap();

        assert!(post_swap_called);
        assert!(!Migrate::has_pending_sst_rebuild(&db_path));
        assert!(!migrating_path.exists());
        assert_eq!(RocksDB::detect_schema(&db_path).unwrap(), Some(Schema::V1));
        assert_eq!(
            RocksDB::detect_schema(&backup_path).unwrap(),
            Some(Schema::Legacy)
        );
    }

    // Case C: active DB is V1 and migrating dir is absent, but old legacy backup is missing;
    // recovery must refuse and keep the cutover marker intact.
    {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_sst_rebuild_recover_missing_backup")
            .tempdir()
            .unwrap();
        let db_path = tmp_dir.as_ref().join("db");
        drop(create_minimal_legacy_db(&db_path));
        SstRebuild::new(db_path.clone()).unwrap().run().unwrap();

        let backup_path = std::fs::read_dir(tmp_dir.as_ref())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("db.pre-sst-rebuild-")
            })
            .unwrap();
        std::fs::remove_dir_all(&backup_path).unwrap();

        let marker_path = tmp_dir.as_ref().join("db.sst-rebuild-promoting");
        std::fs::write(
            &marker_path,
            format!(
                "db.migrating\n{}\n",
                backup_path.file_name().unwrap().to_string_lossy()
            ),
        )
        .unwrap();

        let mut post_swap_called = false;
        let err = SstRebuild::new(db_path.clone())
            .unwrap()
            .run_with_post_swap(|| {
                post_swap_called = true;
                Ok(())
            })
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("cannot automatically recover interrupted SST rebuild cutover"),
            "{err}"
        );
        assert!(!post_swap_called);
        assert!(Migrate::has_pending_sst_rebuild(&db_path));
    }

    // Case D: active V1 + legacy backup present, but post_swap fails (e.g. config sync failure);
    // marker must remain until a subsequent recovery succeeds.
    {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_sst_rebuild_recover_post_swap_retry")
            .tempdir()
            .unwrap();
        let db_path = tmp_dir.as_ref().join("db");
        drop(create_minimal_legacy_db(&db_path));
        SstRebuild::new(db_path.clone()).unwrap().run().unwrap();

        let backup_path = std::fs::read_dir(tmp_dir.as_ref())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("db.pre-sst-rebuild-")
            })
            .unwrap();

        let marker_path = tmp_dir.as_ref().join("db.sst-rebuild-promoting");
        std::fs::write(
            &marker_path,
            format!(
                "db.migrating\n{}\n",
                backup_path.file_name().unwrap().to_string_lossy()
            ),
        )
        .unwrap();

        let err = SstRebuild::new(db_path.clone())
            .unwrap()
            .run_with_post_swap(|| {
                Err(ckb_error::InternalErrorKind::Config
                    .other("simulated config dir sync error")
                    .into())
            })
            .unwrap_err();
        assert!(err.to_string().contains("simulated config dir sync error"));
        assert!(Migrate::has_pending_sst_rebuild(&db_path));

        SstRebuild::new(db_path.clone())
            .unwrap()
            .run_with_post_swap(|| Ok(()))
            .unwrap();
        assert!(!Migrate::has_pending_sst_rebuild(&db_path));
    }
}

#[test]
fn test_open_bulk_load_db_preserves_v1_schema() {
    let tmp_dir = tempfile::Builder::new()
        .prefix("test_open_bulk_load_db_preserves_v1_schema")
        .tempdir()
        .unwrap();
    let db_path = tmp_dir.as_ref().join("db");
    drop(RocksDB::open_in(&db_path, Schema::V1));

    let mg = Migrate::new(&db_path, HardForks::new_dev());
    let db = mg.open_bulk_load_db().unwrap().unwrap();
    drop(db);
    assert_eq!(RocksDB::detect_schema(&db_path).unwrap(), Some(Schema::V1));
}

#[test]
fn test_sst_rebuild_ingests_to_bottommost_l6_and_guards_paths_and_cutover_lock() {
    use ckb_db::internal::ops::{GetColumnFamilys, GetPropertyCF};

    let tmp_dir = tempfile::Builder::new()
        .prefix("test_sst_rebuild_l6_and_guards")
        .tempdir()
        .unwrap();
    let db_path = tmp_dir.as_ref().join("db");
    drop(create_minimal_legacy_db(&db_path));

    // Symlinked db.path must be rejected upfront before rebuilding.
    #[cfg(unix)]
    {
        let symlink_path = tmp_dir.as_ref().join("db_link");
        std::os::unix::fs::symlink(&db_path, &symlink_path).unwrap();
        let err = SstRebuild::new(symlink_path)
            .unwrap()
            .preflight()
            .unwrap_err()
            .to_string();
        assert!(err.contains("symlink"), "unexpected error: {err}");
    }

    // Holding the sibling advisory lock must block concurrent rebuild/recovery.
    let lock_path = tmp_dir.as_ref().join("db.sst-rebuild.lock");
    {
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();
        lock_file.try_lock().unwrap();
        let err = SstRebuild::new(db_path.clone())
            .unwrap()
            .run()
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("already running"),
            "unexpected concurrent lock error: {err}"
        );
    }

    // Trailing `/.` path normalizes cleanly and completes rebuild; lock file stays on disk
    // while the advisory lock is released so concurrent processes always lock the same inode.
    SstRebuild::new(db_path.join(".")).unwrap().run().unwrap();
    assert!(lock_path.exists());
    assert!(Migrate::acquire_shared_lock(&db_path).is_ok());
    assert!(!Migrate::has_pending_sst_rebuild(&db_path));

    // Ingested SSTs must land directly at bottommost level L6 (0 files at L0..L5).
    let reopened = RocksDB::open_existing_for_validation(&db_path, Schema::V1).unwrap();
    let inner = reopened.inner();
    for col in [COLUMN_BLOCK_HEADER, COLUMN_BLOCK_BODY, COLUMN_HASH_INDEX] {
        let cf = inner.cf_handle(col).unwrap();
        for lvl in 0..6 {
            let count = inner
                .property_int_value_cf(cf, &format!("rocksdb.num-files-at-level{lvl}"))
                .unwrap()
                .unwrap_or(0);
            assert_eq!(count, 0, "expected 0 SST files at L{lvl} for column {col}");
        }
        let l6 = inner
            .property_int_value_cf(cf, "rocksdb.num-files-at-level6")
            .unwrap()
            .unwrap_or(0);
        assert!(l6 > 0, "expected ingested SST files at L6 for column {col}");
    }
}

#[test]
fn test_sst_rebuild_migrates_synced_unflushed_wal_and_rejects_separate_wal_dir() {
    // Part A: Synced writes that stay in the WAL (not flushed to SST) must be migrated.
    {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_sst_rebuild_unflushed_wal")
            .tempdir()
            .unwrap();
        let db_path = tmp_dir.as_ref().join("db");
        let opts_file = tmp_dir.as_ref().join("unflushed.db-options");
        std::fs::write(
            &opts_file,
            "[DBOptions]\navoid_flush_during_shutdown=true\n\n[CFOptions \"default\"]\nwrite_buffer_size=67108864\n",
        )
        .unwrap();

        let genesis = BlockBuilder::default()
            .transaction(
                TransactionBuilder::default()
                    .witness(Bytes::default())
                    .build(),
            )
            .build_unchecked();
        let block1 = BlockBuilder::default()
            .header(
                HeaderBuilder::default()
                    .parent_hash(genesis.hash())
                    .number(1)
                    .epoch(EpochNumberWithFraction::new(0, 1, 10))
                    .nonce(7)
                    .build(),
            )
            .transaction(
                TransactionBuilder::default()
                    .witness(Bytes::from(vec![0xAB; 64]))
                    .build(),
            )
            .build_unchecked();

        {
            let config = DBConfig {
                path: db_path.clone(),
                options_file: Some(opts_file.clone()),
                ..Default::default()
            };
            let db = RocksDB::open(&config, Schema::Legacy);

            // Persist genesis and flush default/header SSTs first, then write block1 + tip ONLY
            // via write_sync without flushing.
            let txn = db.transaction();
            insert_old_layout_block(&txn, &genesis);
            let zero_key: packed::Uint64 = 0u64.into();
            txn.put(
                legacy::COLUMN_INDEX,
                zero_key.as_slice(),
                genesis.hash().as_slice(),
            )
            .unwrap();
            txn.put(
                legacy::COLUMN_META,
                META_TIP_HEADER_KEY,
                genesis.hash().as_slice(),
            )
            .unwrap();
            txn.commit().unwrap();
            drop(txn);
            db.put_default_sync(MIGRATION_VERSION_KEY, b"20231101000000")
                .unwrap();

            let mut batch = db.new_write_batch();
            let header1: packed::HeaderView = block1.header().into();
            let uncles1: packed::UncleBlockVecView = block1.uncles().into();
            let proposals1 = block1.data().proposals();
            let ext1 = BlockExt {
                received_at: unix_time_as_millis(),
                total_difficulty: block1.difficulty(),
                total_uncles_count: 0,
                verified: Some(true),
                txs_fees: vec![],
                cycles: None,
                txs_sizes: None,
            };
            batch
                .put(
                    legacy::COLUMN_BLOCK_HEADER,
                    block1.hash().as_slice(),
                    header1.as_slice(),
                )
                .unwrap();
            batch
                .put(
                    legacy::COLUMN_BLOCK_UNCLE,
                    block1.hash().as_slice(),
                    uncles1.as_slice(),
                )
                .unwrap();
            batch
                .put(
                    legacy::COLUMN_BLOCK_PROPOSAL_IDS,
                    block1.hash().as_slice(),
                    proposals1.as_slice(),
                )
                .unwrap();
            batch
                .put(
                    legacy::COLUMN_BLOCK_EXT,
                    block1.hash().as_slice(),
                    Into::<packed::BlockExtV1>::into(ext1).as_slice(),
                )
                .unwrap();
            for (index, tx) in block1.transactions().into_iter().enumerate() {
                let key = packed::TransactionKey::new_builder()
                    .block_hash(block1.hash())
                    .index(index)
                    .build();
                let tx_data = Into::<packed::TransactionView>::into(tx);
                batch
                    .put(
                        legacy::COLUMN_BLOCK_BODY,
                        key.as_slice(),
                        tx_data.as_slice(),
                    )
                    .unwrap();
            }
            let one_key: packed::Uint64 = 1u64.into();
            batch
                .put(
                    legacy::COLUMN_INDEX,
                    one_key.as_slice(),
                    block1.hash().as_slice(),
                )
                .unwrap();
            batch
                .put(
                    legacy::COLUMN_META,
                    META_TIP_HEADER_KEY,
                    block1.hash().as_slice(),
                )
                .unwrap();
            db.write_sync(&batch).unwrap();
        }

        // With avoid_flush_during_shutdown=true, data must still live in non-empty WAL .log files
        // with zero SST files written for block1.
        let has_non_empty_wal = std::fs::read_dir(&db_path)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry.path().extension().and_then(|s| s.to_str()) == Some("log")
                    && entry.metadata().map(|m| m.len() > 0).unwrap_or(false)
            });
        let has_sst = std::fs::read_dir(&db_path)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.path().extension().and_then(|s| s.to_str()) == Some("sst"));
        assert!(has_non_empty_wal, "expected non-empty synced WAL file");
        assert!(!has_sst, "expected writes to remain unflushed in WAL");

        SstRebuild::new(db_path.clone())
            .unwrap()
            .with_options_file(opts_file)
            .run()
            .unwrap();

        let migrated = RocksDB::open_in(&db_path, Schema::V1);
        assert_eq!(
            migrated
                .get_pinned(COLUMN_INDEX, &block_number_key(1))
                .unwrap()
                .as_deref(),
            Some(block1.hash().as_slice())
        );
        assert!(
            migrated
                .get_pinned(COLUMN_BLOCK_HEADER, &block_key(1, &block1.hash()))
                .unwrap()
                .is_some()
        );
        assert!(
            migrated
                .get_pinned(COLUMN_BLOCK_BODY, &block_body_key(1, &block1.hash(), 0))
                .unwrap()
                .is_some()
        );
    }

    // Part B: A separate wal_dir configured in options_file must be rejected before any
    // writable open so synced WAL writes in the external directory cannot be silently skipped.
    {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_sst_rebuild_separate_wal_dir")
            .tempdir()
            .unwrap();
        let db_path = tmp_dir.as_ref().join("db");
        let wal_dir = tmp_dir.as_ref().join("external_wal");
        std::fs::create_dir_all(&wal_dir).unwrap();
        let escape_opt_path = |p: &std::path::Path| p.to_string_lossy().replace('\\', "\\\\");
        let opts_file = tmp_dir.as_ref().join("separate-wal.db-options");
        std::fs::write(
            &opts_file,
            format!(
                "[DBOptions]\nwal_dir={}\navoid_flush_during_shutdown=true\n\n[CFOptions \"default\"]\nwrite_buffer_size=67108864\n",
                escape_opt_path(&wal_dir)
            ),
        )
        .unwrap();

        let config = DBConfig {
            path: db_path.clone(),
            options_file: Some(opts_file.clone()),
            ..Default::default()
        };
        let genesis = BlockBuilder::default()
            .transaction(
                TransactionBuilder::default()
                    .witness(Bytes::default())
                    .build(),
            )
            .build_unchecked();
        {
            let db = RocksDB::open(&config, Schema::Legacy);
            let txn = db.transaction();
            insert_old_layout_block(&txn, &genesis);
            let zero_key: packed::Uint64 = 0u64.into();
            txn.put(
                legacy::COLUMN_INDEX,
                zero_key.as_slice(),
                genesis.hash().as_slice(),
            )
            .unwrap();
            txn.put(
                legacy::COLUMN_META,
                META_TIP_HEADER_KEY,
                genesis.hash().as_slice(),
            )
            .unwrap();
            txn.commit().unwrap();
            drop(txn);
            db.put_default_sync(MIGRATION_VERSION_KEY, b"20231101000000")
                .unwrap();
        }

        let preflight_err = SstRebuild::new(db_path.clone())
            .unwrap()
            .with_options_file(opts_file.clone())
            .preflight()
            .unwrap_err()
            .to_string();
        assert!(
            preflight_err.contains("separate wal_dir"),
            "unexpected preflight error: {preflight_err}"
        );

        let run_err = SstRebuild::new(db_path.clone())
            .unwrap()
            .with_options_file(opts_file.clone())
            .run()
            .unwrap_err()
            .to_string();
        assert!(
            run_err.contains("separate wal_dir"),
            "unexpected run error: {run_err}"
        );

        // `ckb migrate --force` (`open_bulk_load_db`) must also reject separate wal_dir before
        // opening the database writable, while read-only inspection honors options_file.
        let mg = Migrate::new(&db_path, HardForks::new_dev()).with_options_file(Some(opts_file));
        assert!(mg.open_read_only_db().unwrap().is_some());
        let bulk_err = mg.open_bulk_load_db().err().unwrap().to_string();
        assert!(
            bulk_err.contains("separate wal_dir"),
            "unexpected bulk_load error: {bulk_err}"
        );

        // Duplicate `wal_dir` entries follow RocksDB's first-occurrence-wins semantics:
        // a later empty `wal_dir=` must NOT hide an earlier external `wal_dir`.
        let dup_wal_opts = tmp_dir.as_ref().join("dup-wal.db-options");
        std::fs::write(
            &dup_wal_opts,
            format!(
                "[DBOptions]\nwal_dir={}\nwal_dir=\n\n[CFOptions \"default\"]\nwrite_buffer_size=67108864\n",
                escape_opt_path(&wal_dir)
            ),
        )
        .unwrap();
        let dup_preflight_err = SstRebuild::new(db_path.clone())
            .unwrap()
            .with_options_file(dup_wal_opts.clone())
            .preflight()
            .unwrap_err()
            .to_string();
        assert!(
            dup_preflight_err.contains("separate wal_dir"),
            "unexpected duplicate wal_dir preflight error: {dup_preflight_err}"
        );
        let dup_bulk_err = Migrate::new(&db_path, HardForks::new_dev())
            .with_options_file(Some(dup_wal_opts))
            .open_bulk_load_db()
            .err()
            .unwrap()
            .to_string();
        assert!(
            dup_bulk_err.contains("separate wal_dir"),
            "unexpected duplicate wal_dir bulk_load error: {dup_bulk_err}"
        );

        // Trailing Unicode U+00A0 (NO-BREAK SPACE) is preserved by RocksDB's C-locale `isspace`
        // and therefore points to a distinct WAL directory from `db_path`; both `--sst-rebuild`
        // and `--force` (`open_bulk_load_db`) must reject it.
        let nbsp_wal_opts = tmp_dir.as_ref().join("nbsp-wal.db-options");
        std::fs::write(
            &nbsp_wal_opts,
            format!(
                "[DBOptions]\nwal_dir={}\u{00A0}\n\n[CFOptions \"default\"]\nwrite_buffer_size=67108864\n",
                escape_opt_path(&db_path)
            ),
        )
        .unwrap();
        let nbsp_preflight_err = SstRebuild::new(db_path.clone())
            .unwrap()
            .with_options_file(nbsp_wal_opts.clone())
            .preflight()
            .unwrap_err()
            .to_string();
        assert!(
            nbsp_preflight_err.contains("separate wal_dir"),
            "unexpected U+00A0 wal_dir preflight error: {nbsp_preflight_err}"
        );
        let nbsp_bulk_err = Migrate::new(&db_path, HardForks::new_dev())
            .with_options_file(Some(nbsp_wal_opts))
            .open_bulk_load_db()
            .err()
            .unwrap()
            .to_string();
        assert!(
            nbsp_bulk_err.contains("separate wal_dir"),
            "unexpected U+00A0 wal_dir bulk_load error: {nbsp_bulk_err}"
        );

        // Explicit `wal_dir` pointing to `db_path` itself (without external suffix) must pass.
        let same_wal_opts = tmp_dir.as_ref().join("same-wal.db-options");
        std::fs::write(
            &same_wal_opts,
            format!(
                "[DBOptions]\nwal_dir={}\n\n[CFOptions \"default\"]\nwrite_buffer_size=67108864\n",
                escape_opt_path(&db_path)
            ),
        )
        .unwrap();
        RocksDB::check_options_file_wal_dir(&db_path, &same_wal_opts).unwrap();

        // Conversely, an initial empty `wal_dir=` followed by an external `wal_dir` resolves
        // to empty in RocksDB and must be accepted.
        let empty_first_wal_opts = tmp_dir.as_ref().join("empty-first-wal.db-options");
        std::fs::write(
            &empty_first_wal_opts,
            format!(
                "[DBOptions]\nwal_dir=\nwal_dir={}\n\n[CFOptions \"default\"]\nwrite_buffer_size=67108864\n",
                escape_opt_path(&wal_dir)
            ),
        )
        .unwrap();
        RocksDB::check_options_file_wal_dir(&db_path, &empty_first_wal_opts).unwrap();

        assert!(!tmp_dir.as_ref().join("db.migrating").exists());
        assert!(!Migrate::has_pending_sst_rebuild(&db_path));
        assert_eq!(
            RocksDB::detect_schema(&db_path).unwrap(),
            Some(Schema::Legacy)
        );
    }
}

#[test]
fn test_sst_rebuild_rejects_incompatible_v1_num_levels_before_cutover() {
    let tmp_dir = tempfile::Builder::new()
        .prefix("test_sst_rebuild_num_levels_4")
        .tempdir()
        .unwrap();
    let db_path = tmp_dir.as_ref().join("db");
    drop(create_minimal_legacy_db(&db_path));

    let low_default_opts = tmp_dir.as_ref().join("default-4.db-options");
    std::fs::write(
        &low_default_opts,
        "[DBOptions]\nmax_background_jobs=4\n\n[CFOptions \"default\"]\nnum_levels=4\n",
    )
    .unwrap();

    // The options file is syntactically V1-valid, yet incompatible with an L6 rebuilt DB.
    RocksDB::validate_options_file(&low_default_opts, Schema::V1).unwrap();

    let preflight_err = SstRebuild::new(db_path.clone())
        .unwrap()
        .with_options_file(low_default_opts.clone())
        .preflight()
        .unwrap_err()
        .to_string();
    assert!(
        preflight_err.contains("num_levels=4"),
        "unexpected preflight error: {preflight_err}"
    );

    let run_err = SstRebuild::new(db_path.clone())
        .unwrap()
        .with_options_file(low_default_opts.clone())
        .run()
        .unwrap_err()
        .to_string();
    assert!(
        run_err.contains("num_levels=4"),
        "unexpected run error: {run_err}"
    );
    assert_eq!(
        RocksDB::detect_schema(&db_path).unwrap(),
        Some(Schema::Legacy)
    );

    // Per-CF num_levels < 7 override must also be rejected upfront.
    let low_cf_opts = tmp_dir.as_ref().join("cf-4.db-options");
    std::fs::write(
        &low_cf_opts,
        format!(
            "[DBOptions]\nmax_background_jobs=4\n\n[CFOptions \"default\"]\nnum_levels=7\n\n[CFOptions \"{COLUMN_BLOCK_HEADER}\"]\nnum_levels=4\n"
        ),
    )
    .unwrap();
    let cf_err = SstRebuild::new(db_path.clone())
        .unwrap()
        .with_options_file(low_cf_opts)
        .preflight()
        .unwrap_err()
        .to_string();
    assert!(
        cf_err.contains("num_levels=4") && cf_err.contains(COLUMN_BLOCK_HEADER),
        "unexpected per-CF error: {cf_err}"
    );

    // Duplicate `num_levels` entries also follow RocksDB's first-occurrence-wins semantics.
    let dup_levels_opts = tmp_dir.as_ref().join("dup-levels.db-options");
    std::fs::write(
        &dup_levels_opts,
        "[DBOptions]\nmax_background_jobs=4\n\n[CFOptions \"default\"]\nnum_levels=4\nnum_levels=7\n",
    )
    .unwrap();
    let dup_levels_err = SstRebuild::new(db_path.clone())
        .unwrap()
        .with_options_file(dup_levels_opts)
        .preflight()
        .unwrap_err()
        .to_string();
    assert!(
        dup_levels_err.contains("num_levels=4"),
        "unexpected duplicate num_levels error: {dup_levels_err}"
    );

    // Even at target reopen time, RocksDB itself rejects opening an L6 DB with num_levels=4.
    SstRebuild::new(db_path.clone()).unwrap().run().unwrap();
    assert!(
        RocksDB::open_existing_for_validation_with_options(
            &db_path,
            Schema::V1,
            Some(&low_default_opts)
        )
        .is_err()
    );
}

#[test]
fn test_sst_rebuild_shared_lock_coordinates_with_startup_and_cutover() {
    let tmp_dir = tempfile::Builder::new()
        .prefix("test_sst_rebuild_shared_lock")
        .tempdir()
        .unwrap();
    let db_path = tmp_dir.as_ref().join("db");
    drop(create_minimal_legacy_db(&db_path));

    // Multiple shared locks (e.g., startup/checker) can coexist and block exclusive rebuild.
    let shared_1 = Migrate::acquire_shared_lock(&db_path).unwrap();
    let shared_2 = Migrate::acquire_shared_lock(&db_path).unwrap();
    let err = SstRebuild::new(db_path.clone())
        .unwrap()
        .run()
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("already running"),
        "expected rebuild to block on shared lock: {err}"
    );
    drop(shared_1);
    drop(shared_2);

    // While rebuild holds the exclusive lock (through cutover and post_swap), startup shared lock
    // must fail immediately.
    let mut checked_during_post_swap = false;
    SstRebuild::new(db_path.clone())
        .unwrap()
        .run_with_post_swap(|| {
            let err = Migrate::acquire_shared_lock(&db_path)
                .err()
                .expect("shared lock must fail during rebuild")
                .to_string();
            assert!(
                err.contains("currently in progress"),
                "unexpected shared lock error: {err}"
            );
            checked_during_post_swap = true;
            Ok(())
        })
        .unwrap();
    assert!(checked_during_post_swap);

    // After rebuild completes, the lock file remains on disk and shared lock succeeds again.
    assert!(tmp_dir.as_ref().join("db.sst-rebuild.lock").exists());
    drop(Migrate::acquire_shared_lock(&db_path).unwrap());
}

fn create_minimal_legacy_db(db_path: &std::path::Path) -> RocksDB {
    let db = RocksDB::open_in(db_path, Schema::Legacy);
    let genesis = BlockBuilder::default()
        .transaction(
            TransactionBuilder::default()
                .witness(Bytes::default())
                .build(),
        )
        .build_unchecked();
    let txn = db.transaction();
    insert_old_layout_block(&txn, &genesis);
    let zero_key: packed::Uint64 = 0u64.into();
    txn.put(
        legacy::COLUMN_INDEX,
        zero_key.as_slice(),
        genesis.hash().as_slice(),
    )
    .unwrap();
    txn.put(
        legacy::COLUMN_META,
        META_TIP_HEADER_KEY,
        genesis.hash().as_slice(),
    )
    .unwrap();
    txn.commit().unwrap();
    drop(txn);
    db.put_default(MIGRATION_VERSION_KEY, b"20231101000000")
        .unwrap();
    db
}

fn insert_old_layout_block(
    db_txn: &ckb_db::RocksDBTransaction,
    block: &ckb_types::core::BlockView,
) {
    let hash = block.hash();
    let header: packed::HeaderView = block.header().into();
    let uncles: packed::UncleBlockVecView = block.uncles().into();
    let proposals = block.data().proposals();
    let ext = BlockExt {
        received_at: unix_time_as_millis(),
        total_difficulty: block.difficulty(),
        total_uncles_count: 0,
        verified: None,
        txs_fees: vec![],
        cycles: None,
        txs_sizes: None,
    };

    db_txn
        .put(
            legacy::COLUMN_BLOCK_HEADER,
            hash.as_slice(),
            header.as_slice(),
        )
        .unwrap();
    db_txn
        .put(
            legacy::COLUMN_BLOCK_UNCLE,
            hash.as_slice(),
            uncles.as_slice(),
        )
        .unwrap();
    db_txn
        .put(
            legacy::COLUMN_BLOCK_PROPOSAL_IDS,
            hash.as_slice(),
            proposals.as_slice(),
        )
        .unwrap();
    db_txn
        .put(
            legacy::COLUMN_BLOCK_EXT,
            hash.as_slice(),
            Into::<packed::BlockExtV1>::into(ext).as_slice(),
        )
        .unwrap();

    for (index, tx) in block.transactions().into_iter().enumerate() {
        let key = packed::TransactionKey::new_builder()
            .block_hash(hash.clone())
            .index(index)
            .build();
        let tx_data = Into::<packed::TransactionView>::into(tx);
        db_txn
            .put(
                legacy::COLUMN_BLOCK_BODY,
                key.as_slice(),
                tx_data.as_slice(),
            )
            .unwrap();
    }
}
