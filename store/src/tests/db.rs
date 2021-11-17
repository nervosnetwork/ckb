use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_db::RocksDB;
use ckb_db_schema::{COLUMNS, COLUMN_BLOCK_HEADER};
use ckb_freezer::Freezer;
use ckb_types::{core::BlockExt, packed, prelude::*};
use tempfile::TempDir;

use crate::{db::ChainDB, store::ChainStore};

#[test]
fn save_and_get_block() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let store = ChainDB::new(db, Default::default());
    let consensus = ConsensusBuilder::default().build();
    let block = consensus.genesis_block();

    let hash = block.hash();
    let txn = store.begin_transaction();
    txn.insert_block(&block).unwrap();
    txn.commit().unwrap();
    assert_eq!(block, &store.get_block(&hash).unwrap());
}

#[test]
fn save_and_get_block_with_transactions() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let store = ChainDB::new(db, Default::default());
    let block = packed::Block::new_builder()
        .transactions(
            (0..3)
                .map(|_| packed::Transaction::new_builder().build())
                .collect::<Vec<_>>()
                .pack(),
        )
        .build()
        .into_view();

    let hash = block.hash();
    let txn = store.begin_transaction();
    txn.insert_block(&block).unwrap();
    txn.commit().unwrap();
    assert_eq!(block, store.get_block(&hash).unwrap());
}

#[test]
fn save_and_get_block_ext() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let store = ChainDB::new(db, Default::default());
    let consensus = ConsensusBuilder::default().build();
    let block = consensus.genesis_block();

    let ext = BlockExt {
        received_at: block.timestamp(),
        total_difficulty: block.difficulty(),
        total_uncles_count: block.data().uncles().len() as u64,
        verified: Some(true),
        txs_fees: vec![],
    };

    let hash = block.hash();
    let txn = store.begin_transaction();
    txn.insert_block_ext(&hash, &ext).unwrap();
    txn.commit().unwrap();
    assert_eq!(ext, store.get_block_ext(&hash).unwrap());
}

#[test]
fn index_store() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let store = ChainDB::new(db, Default::default());
    let consensus = ConsensusBuilder::default().build();
    let block = consensus.genesis_block();
    let hash = block.hash();
    store.init(&consensus).unwrap();
    assert_eq!(hash, store.get_block_hash(0).unwrap());

    assert_eq!(
        block.difficulty(),
        store.get_block_ext(&hash).unwrap().total_difficulty
    );

    assert_eq!(block.number(), store.get_block_number(&hash).unwrap());

    assert_eq!(block.header(), store.get_tip_header().unwrap());
}

#[test]
fn freeze_blockv0() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let tmp_dir2 = TempDir::new().unwrap();
    let freezer = Freezer::open_in(&tmp_dir2).expect("tmp freezer");
    let store = ChainDB::new_with_freezer(db, freezer.clone(), Default::default());

    let raw = packed::RawHeader::new_builder().number(1u64.pack()).build();
    let block = packed::Block::new_builder()
        .header(packed::Header::new_builder().raw(raw).build())
        .build()
        .into_view();

    let block_hash = block.hash();
    let header = block.header();

    let txn = store.begin_transaction();
    txn.insert_raw(
        COLUMN_BLOCK_HEADER,
        block_hash.as_slice(),
        header.pack().as_slice(),
    )
    .expect("insert header");
    txn.commit().expect("commit");

    freezer
        .freeze(2, |_number| Some(block.clone()))
        .expect("freeze");

    assert_eq!(store.get_block(&block_hash), Some(block));
}

#[test]
fn freeze_blockv1_with_extension() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let tmp_dir2 = TempDir::new().unwrap();
    let freezer = Freezer::open_in(&tmp_dir2).expect("tmp freezer");
    let store = ChainDB::new_with_freezer(db, freezer.clone(), Default::default());

    let extension: packed::Bytes = vec![1u8; 96].pack();
    let raw = packed::RawHeader::new_builder().number(1u64.pack()).build();
    let block = packed::BlockV1::new_builder()
        .header(packed::Header::new_builder().raw(raw).build())
        .extension(extension)
        .build()
        .as_v0()
        .into_view();

    let block_hash = block.hash();
    let header = block.header();

    let txn = store.begin_transaction();
    txn.insert_raw(
        COLUMN_BLOCK_HEADER,
        block_hash.as_slice(),
        header.pack().as_slice(),
    )
    .expect("insert header");
    txn.commit().expect("commit");

    freezer
        .freeze(2, |_number| Some(block.clone()))
        .expect("freeze");

    let block = store.get_block(&block_hash).expect("get_block");
    assert_eq!(store.get_block(&block_hash), Some(block));
}
