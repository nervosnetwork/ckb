use crate::migrate::Migrate;
use crate::sst_rebuild::SstRebuild;
use ckb_app_config::DBConfig;
use ckb_chain_spec::consensus::build_genesis_epoch_ext;
use ckb_db::RocksDB;
use ckb_db_schema::{
    COLUMN_BLOCK_BODY, COLUMN_BLOCK_HEADER, COLUMN_HASH_INDEX, COLUMN_INDEX, COLUMNS,
    META_CURRENT_EPOCH_KEY, META_TIP_HEADER_KEY, MIGRATION_VERSION_KEY, legacy,
};
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{
    block_number_to_key,
    core::{
        BlockBuilder, BlockExt, Capacity, HeaderBuilder, TransactionBuilder, capacity_bytes,
        hardfork::HardForks,
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
    let db = RocksDB::open(&config, 12);
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
    let db = RocksDB::open(&config, legacy::COLUMNS);

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

    let migrated = RocksDB::open_in(&db_path, COLUMNS);
    let block1_key = block1.hash().to_block_key(1);
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

    let fork1_key = fork1.hash().to_block_key(1);
    assert!(
        migrated
            .get_pinned(COLUMN_BLOCK_HEADER, &fork1_key)
            .unwrap()
            .is_some()
    );

    let tx_key = block1.hash().to_tx_key(1, 1);
    assert!(
        migrated
            .get_pinned(COLUMN_BLOCK_BODY, &tx_key)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        migrated
            .get_pinned(COLUMN_INDEX, &block_number_to_key(1))
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
        packed::Byte32::number_from_index_value(canonical_index.as_ref()),
        Some(1)
    );
    assert_eq!(
        packed::Byte32::is_main_chain_from_index_value(canonical_index.as_ref()),
        Some(true)
    );

    let fork_index = migrated
        .get_pinned(COLUMN_HASH_INDEX, fork1.hash().as_slice())
        .unwrap()
        .unwrap();
    assert_eq!(
        packed::Byte32::number_from_index_value(fork_index.as_ref()),
        Some(1)
    );
    assert_eq!(
        packed::Byte32::is_main_chain_from_index_value(fork_index.as_ref()),
        Some(false)
    );
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
