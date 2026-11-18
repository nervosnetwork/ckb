use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_db::RocksDB;
use ckb_db_schema::COLUMNS;
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
    txn.insert_block(block).unwrap();
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
        cycles: None,
        txs_sizes: None,
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
    let store = ChainDB::new_with_freezer(db, freezer, Default::default());

    let raw = packed::RawHeader::new_builder().number(1u64).build();
    let block = packed::Block::new_builder()
        .header(packed::Header::new_builder().raw(raw).build())
        .build()
        .into_view();

    let block_hash = block.hash();

    let txn = store.begin_transaction();
    txn.insert_block(&block).expect("insert complete hot block");
    txn.commit().expect("commit");
    drop(txn);

    archive(&store, std::slice::from_ref(&block));

    assert_eq!(store.get_block(&block_hash), Some(block));
}

#[test]
fn freeze_blockv1_with_extension() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let tmp_dir2 = TempDir::new().unwrap();
    let freezer = Freezer::open_in(&tmp_dir2).expect("tmp freezer");
    let store = ChainDB::new_with_freezer(db, freezer, Default::default());

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
    txn.insert_block(&block).expect("insert complete hot block");
    txn.commit().expect("commit");
    drop(txn);

    archive(&store, std::slice::from_ref(&block));

    let block = store.get_block(&block_hash).expect("get_block");
    assert_eq!(store.get_block(&block_hash), Some(block));
}

#[test]
fn freezer_get_block_keeps_hash_lookup_contract_for_same_height_side_block() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let tmp_dir2 = TempDir::new().unwrap();
    let freezer = Freezer::open_in(&tmp_dir2).expect("tmp freezer");
    let store = ChainDB::new_with_freezer(db, freezer, Default::default());

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
    drop(txn);

    archive(&store, std::slice::from_ref(&frozen_block));

    assert_eq!(store.get_block(&side_hash), Some(side_block));
}

#[test]
fn freezer_get_transaction_keeps_hash_lookup_contract_for_same_height_side_tx() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let tmp_dir2 = TempDir::new().unwrap();
    let freezer = Freezer::open_in(&tmp_dir2).expect("tmp freezer");
    let store = ChainDB::new_with_freezer(db, freezer, Default::default());

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
    drop(txn);

    archive(&store, std::slice::from_ref(&frozen_block));

    let (tx, tx_info) = store
        .get_transaction_with_info(&side_tx_hash)
        .expect("get side transaction");
    assert_eq!(tx, side_tx);
    assert_eq!(tx_info.block_hash, side_block_hash);
}

pub(super) fn archive(store: &ChainDB, blocks: &[ckb_types::core::BlockView]) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let controller = ckb_freezer::FreezerController::start(
        store.freezer().unwrap().clone(),
        runtime.handle().clone(),
        ckb_freezer::FreezerServiceConfig::default(),
    )
    .unwrap();
    let txn = store.begin_transaction();
    for block in blocks {
        txn.insert_block_ext(
            &block.hash(),
            &BlockExt {
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
    }
    txn.commit().unwrap();
    drop(txn);
    runtime
        .block_on(controller.append(blocks.iter().map(|block| block.data()).collect()))
        .unwrap();
    assert_eq!(
        store.recover_archive().unwrap(),
        store.freezer().unwrap().number()
    );
    for block in blocks {
        assert!(store.get_archived_block(&block.hash()).is_some());
    }
    runtime.block_on(controller.shutdown()).unwrap();
}
