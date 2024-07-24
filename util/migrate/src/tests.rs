use crate::migrate::Migrate;
use ckb_app_config::DBConfig;
use ckb_chain_spec::consensus::build_genesis_epoch_ext;
use ckb_db::RocksDB;
use ckb_db_schema::{
    COLUMN_BLOCK_BODY, COLUMN_BLOCK_EPOCH, COLUMN_BLOCK_EXT, COLUMN_BLOCK_HEADER,
    COLUMN_BLOCK_PROPOSAL_IDS, COLUMN_BLOCK_UNCLE, COLUMN_EPOCH, COLUMN_INDEX, COLUMN_META,
};
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{
    core::{
        capacity_bytes, hardfork::HardForks, BlockBuilder, BlockExt, Capacity, TransactionBuilder,
    },
    packed::{self, Bytes},
    prelude::*,
    utilities::DIFF_TWO,
};

#[test]
fn test_mock_migration() {
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
        let header = genesis.header().pack();
        let number = header.data().raw().number();
        let uncles = genesis.uncles().pack();
        let proposals = genesis.data().proposals();
        db_txn
            .put(COLUMN_INDEX::NAME, number.as_slice(), hash.as_slice())
            .unwrap();
        db_txn
            .put(
                COLUMN_BLOCK_HEADER::NAME,
                hash.as_slice(),
                header.as_slice(),
            )
            .unwrap();
        db_txn
            .put(COLUMN_BLOCK_UNCLE::NAME, hash.as_slice(), uncles.as_slice())
            .unwrap();
        db_txn
            .put(
                COLUMN_BLOCK_PROPOSAL_IDS::NAME,
                hash.as_slice(),
                proposals.as_slice(),
            )
            .unwrap();
        for (index, tx) in genesis.transactions().into_iter().enumerate() {
            let key = COLUMN_BLOCK_BODY::key(number.unpack(), hash.to_owned(), index);
            let tx_data = tx.pack();
            db_txn
                .put(COLUMN_BLOCK_BODY::NAME, key.as_ref(), tx_data.as_slice())
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
                COLUMN_BLOCK_EPOCH::NAME,
                genesis.header().hash().as_slice(),
                epoch_ext.last_block_hash_in_previous_epoch().as_slice(),
            )
            .unwrap()
    }
    // insert epoch ext
    {
        db_txn
            .put(
                COLUMN_EPOCH::NAME,
                epoch_ext.last_block_hash_in_previous_epoch().as_slice(),
                epoch_ext.pack().as_slice(),
            )
            .unwrap();
        let epoch_number: packed::Uint64 = epoch_ext.number().pack();
        db_txn
            .put(
                COLUMN_EPOCH::NAME,
                epoch_number.as_slice(),
                epoch_ext.last_block_hash_in_previous_epoch().as_slice(),
            )
            .unwrap()
    }

    // insert tip header
    {
        db_txn
            .put(
                COLUMN_META::NAME,
                COLUMN_META::META_TIP_HEADER_KEY,
                genesis.header().hash().as_slice(),
            )
            .unwrap()
    }

    // insert block ext
    {
        db_txn
            .put(
                COLUMN_BLOCK_EXT::NAME,
                COLUMN_BLOCK_EXT::key(genesis.header().num_hash()),
                ext.pack().as_slice(),
            )
            .unwrap()
    }

    // insert_current_epoch_ext
    {
        db_txn
            .put(
                COLUMN_META::NAME,
                COLUMN_META::META_CURRENT_EPOCH_KEY,
                epoch_ext.pack().as_slice(),
            )
            .unwrap()
    }

    db_txn.commit().unwrap();

    drop(db_txn);
    drop(db);

    let mg = Migrate::new(tmp_dir.as_ref().to_path_buf(), HardForks::new_mirana());

    let db = mg.open_bulk_load_db().unwrap().unwrap();

    mg.migrate(db, false).unwrap();

    let mg2 = Migrate::new(tmp_dir.as_ref().to_path_buf(), HardForks::new_mirana());

    let rdb = mg2.open_read_only_db().unwrap().unwrap();

    assert_eq!(mg2.check(&rdb, true), std::cmp::Ordering::Equal)
}
