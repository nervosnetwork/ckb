use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_db::{RocksDB, Schema};
use ckb_db_schema::{
    COLUMN_BLOCK_EPOCH, COLUMN_BLOCK_EXT, COLUMN_BLOCK_FILTER, COLUMN_BLOCK_FILTER_HASH,
    COLUMN_HASH_INDEX, block_key,
};
use ckb_freezer::Freezer;
use ckb_types::{core::BlockExt, packed, prelude::*};
use tempfile::TempDir;

use crate::{db::ChainDB, store::ChainStore};

#[test]
fn save_and_get_block() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, Schema::V1);
    let store = ChainDB::new(db, Default::default());
    let consensus = ConsensusBuilder::default().build();
    let block = consensus.genesis_block();

    let hash = block.hash();
    let txn = store.begin_transaction();
    txn.insert_block(block).unwrap();
    txn.commit().unwrap();
    assert_eq!(block, &store.get_block(&hash).unwrap());
}

#[test]
fn save_and_get_block_with_transactions() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, Schema::V1);
    let store = ChainDB::new(db, Default::default());
    let block = packed::Block::new_builder()
        .transactions(
            (0..3)
                .map(|_| packed::Transaction::new_builder().build())
                .collect::<Vec<_>>(),
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
    let db = RocksDB::open_in(&tmp_dir, Schema::V1);
    let store = ChainDB::new(db, Default::default());
    let consensus = ConsensusBuilder::default().build();
    let block = consensus.genesis_block();

    let ext = BlockExt {
        received_at: block.timestamp(),
        total_difficulty: block.difficulty(),
        total_uncles_count: block.data().uncles().len() as u64,
        verified: Some(true),
        txs_fees: vec![],
        cycles: None,
        txs_sizes: None,
    };

    let hash = block.hash();
    let txn = store.begin_transaction();
    txn.insert_block(block).unwrap();
    txn.insert_block_ext(block.number(), &hash, &ext).unwrap();
    txn.commit().unwrap();
    assert_eq!(ext, store.get_block_ext(&hash).unwrap());
}

#[test]
fn index_store() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, Schema::V1);
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
fn get_block_number_returns_only_main_chain_blocks() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, Schema::V1);
    let store = ChainDB::new(db, Default::default());
    let consensus = ConsensusBuilder::default().build();
    let genesis_hash = consensus.genesis_block().hash();

    store.init(&consensus).unwrap();

    let raw = packed::RawHeader::new_builder()
        .parent_hash(genesis_hash.clone())
        .number(1u64)
        .build();
    let fork = packed::Block::new_builder()
        .header(packed::Header::new_builder().raw(raw).build())
        .transactions(vec![packed::Transaction::new_builder().build()])
        .build()
        .into_view();
    let fork_hash = fork.hash();

    let txn = store.begin_transaction();
    txn.insert_block(&fork).unwrap();
    txn.commit().unwrap();

    assert_eq!(store.get_block_number(&genesis_hash), Some(0));
    assert_eq!(store.get_block_number(&fork_hash), None);
    assert_eq!(store.get_block_header(&fork_hash), Some(fork.header()));
    assert_eq!(
        store.get_cellbase(&fork_hash),
        Some(fork.transactions()[0].clone())
    );
    assert!(store.block_exists(&fork_hash));
}

#[test]
fn delete_block_removes_hash_index_and_block_keyed_metadata() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, Schema::V1);
    let store = ChainDB::new(db, Default::default());
    let consensus = ConsensusBuilder::default().build();
    let genesis_hash = consensus.genesis_block().hash();

    store.init(&consensus).unwrap();

    let raw = packed::RawHeader::new_builder()
        .parent_hash(genesis_hash)
        .number(1u64)
        .build();
    let block = packed::Block::new_builder()
        .header(packed::Header::new_builder().raw(raw).build())
        .build()
        .into_view();
    let block_hash = block.hash();
    let block_key = block_key(block.number(), &block_hash);
    let ext = BlockExt {
        received_at: block.timestamp(),
        total_difficulty: block.difficulty(),
        total_uncles_count: block.data().uncles().len() as u64,
        verified: Some(false),
        txs_fees: vec![],
        cycles: None,
        txs_sizes: None,
    };

    let txn = store.begin_transaction();
    txn.insert_block(&block).unwrap();
    txn.attach_block(&block).unwrap();
    txn.insert_block_ext(block.number(), &block_hash, &ext)
        .unwrap();
    txn.insert_raw(COLUMN_BLOCK_EPOCH, &block_key, b"epoch")
        .unwrap();
    txn.insert_raw(COLUMN_BLOCK_FILTER, &block_key, b"filter")
        .unwrap();
    txn.insert_raw(COLUMN_BLOCK_FILTER_HASH, &block_key, b"filter_hash")
        .unwrap();
    txn.commit().unwrap();

    assert!(store.block_exists(&block_hash));
    assert_eq!(store.get_block_number(&block_hash), Some(block.number()));
    assert_eq!(
        store.get_block_hash(block.number()),
        Some(block_hash.clone())
    );
    assert_eq!(store.get_block_ext(&block_hash), Some(ext));

    let txn = store.begin_transaction();
    txn.delete_block(&block).unwrap();
    txn.commit().unwrap();

    assert!(!store.block_exists(&block_hash));
    assert_eq!(store.get_block(&block_hash), None);
    assert_eq!(store.get_block_number(&block_hash), None);
    assert_eq!(store.get_block_hash(block.number()), None);
    assert_eq!(store.get_block_ext(&block_hash), None);
    assert!(
        store
            .get(COLUMN_HASH_INDEX, block_hash.as_slice())
            .is_none()
    );
    assert!(store.get(COLUMN_BLOCK_EXT, &block_key).is_none());
    assert!(store.get(COLUMN_BLOCK_EPOCH, &block_key).is_none());
    assert!(store.get(COLUMN_BLOCK_FILTER, &block_key).is_none());
    assert!(store.get(COLUMN_BLOCK_FILTER_HASH, &block_key).is_none());
}

#[test]
fn get_transaction_from_initialized_store() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, Schema::V1);
    let store = ChainDB::new(db, Default::default());
    let consensus = ConsensusBuilder::default().build();
    let block = consensus.genesis_block();
    let block_hash = block.hash();
    let tx = block.transactions()[0].clone();
    let tx_hash = tx.hash();

    store.init(&consensus).unwrap();

    let (found_tx, found_block_hash) = store.get_transaction(&tx_hash).unwrap();
    assert_eq!(found_block_hash, block_hash);
    assert_eq!(found_tx, tx);
}

#[test]
fn freeze_blockv0() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, Schema::V1);
    let tmp_dir2 = TempDir::new().unwrap();
    let freezer = Freezer::open_in(&tmp_dir2).expect("tmp freezer");
    let store = ChainDB::new_with_freezer(db, freezer.clone(), Default::default());

    let raw = packed::RawHeader::new_builder().number(1u64).build();
    let block = packed::Block::new_builder()
        .header(packed::Header::new_builder().raw(raw).build())
        .build()
        .into_view();

    let block_hash = block.hash();

    let txn = store.begin_transaction();
    txn.insert_block(&block).expect("insert block");
    txn.commit().expect("commit");

    freezer
        .freeze(2, |_number| Some(block.clone()))
        .expect("freeze");

    assert_eq!(store.get_block(&block_hash), Some(block));
}

#[test]
fn freeze_blockv1_with_extension() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, Schema::V1);
    let tmp_dir2 = TempDir::new().unwrap();
    let freezer = Freezer::open_in(&tmp_dir2).expect("tmp freezer");
    let store = ChainDB::new_with_freezer(db, freezer.clone(), Default::default());

    let extension: packed::Bytes = [1u8; 96].into();
    let raw = packed::RawHeader::new_builder().number(1u64).build();
    let block = packed::BlockV1::new_builder()
        .header(packed::Header::new_builder().raw(raw).build())
        .extension(extension)
        .build()
        .as_v0()
        .into_view();

    let block_hash = block.hash();

    let txn = store.begin_transaction();
    txn.insert_block(&block).expect("insert block");
    txn.commit().expect("commit");

    freezer
        .freeze(2, |_number| Some(block.clone()))
        .expect("freeze");

    let block = store.get_block(&block_hash).expect("get_block");
    assert_eq!(store.get_block(&block_hash), Some(block));
}

#[test]
fn freezer_get_block_keeps_hash_lookup_contract_for_same_height_side_block() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, Schema::V1);
    let tmp_dir2 = TempDir::new().unwrap();
    let freezer = Freezer::open_in(&tmp_dir2).expect("tmp freezer");
    let store = ChainDB::new_with_freezer(db, freezer.clone(), Default::default());

    let frozen_block = packed::Block::new_builder()
        .header(
            packed::Header::new_builder()
                .raw(packed::RawHeader::new_builder().number(1u64).build())
                .nonce(1u128)
                .build(),
        )
        .build()
        .into_view();
    let side_block = packed::Block::new_builder()
        .header(
            packed::Header::new_builder()
                .raw(packed::RawHeader::new_builder().number(1u64).build())
                .nonce(2u128)
                .build(),
        )
        .build()
        .into_view();
    let side_hash = side_block.hash();
    assert_ne!(frozen_block.hash(), side_hash);

    let txn = store.begin_transaction();
    txn.insert_block(&frozen_block).unwrap();
    txn.insert_block(&side_block).unwrap();
    txn.commit().unwrap();

    freezer
        .freeze(2, |_number| Some(frozen_block.clone()))
        .expect("freeze");

    assert_eq!(store.get_block(&side_hash), Some(side_block));
}

#[test]
fn freezer_get_transaction_keeps_hash_lookup_contract_for_same_height_side_tx() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, Schema::V1);
    let tmp_dir2 = TempDir::new().unwrap();
    let freezer = Freezer::open_in(&tmp_dir2).expect("tmp freezer");
    let store = ChainDB::new_with_freezer(db, freezer.clone(), Default::default());

    let frozen_tx = packed::Transaction::new_builder()
        .raw(packed::RawTransaction::new_builder().version(1u32).build())
        .build()
        .into_view();
    let side_tx = packed::Transaction::new_builder()
        .raw(packed::RawTransaction::new_builder().version(2u32).build())
        .build()
        .into_view();
    let side_tx_hash = side_tx.hash();
    assert_ne!(frozen_tx.hash(), side_tx_hash);

    let frozen_block = packed::Block::new_builder()
        .header(
            packed::Header::new_builder()
                .raw(packed::RawHeader::new_builder().number(1u64).build())
                .nonce(1u128)
                .build(),
        )
        .transactions(vec![frozen_tx.data()])
        .build();
    let frozen_block = frozen_block.into_view();
    let side_block = packed::Block::new_builder()
        .header(
            packed::Header::new_builder()
                .raw(packed::RawHeader::new_builder().number(1u64).build())
                .nonce(2u128)
                .build(),
        )
        .transactions(vec![side_tx.data()])
        .build();
    let side_block = side_block.into_view();
    let side_block_hash = side_block.hash();
    assert_ne!(frozen_block.hash(), side_block_hash);

    let txn = store.begin_transaction();
    txn.insert_block(&frozen_block).unwrap();
    txn.insert_block(&side_block).unwrap();
    txn.attach_block(&side_block).unwrap();
    txn.commit().unwrap();

    freezer
        .freeze(2, |_number| Some(frozen_block.clone()))
        .expect("freeze");

    let (tx, tx_info) = store
        .get_transaction_with_info(&side_tx_hash)
        .expect("get side transaction");
    assert_eq!(tx, side_tx);
    assert_eq!(tx_info.block_hash, side_block_hash);
}
