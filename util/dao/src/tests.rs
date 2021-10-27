use ckb_chain_spec::consensus::Consensus;
use ckb_dao_utils::{extract_dao_data, pack_dao_data};
use ckb_db::RocksDB;
use ckb_db_schema::COLUMNS;
use ckb_store::{ChainDB, ChainStore};
use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes,
        cell::{CellMetaBuilder, ResolvedTransaction},
        BlockBuilder, BlockNumber, Capacity, EpochExt, HeaderBuilder, HeaderView,
        TransactionBuilder,
    },
    h256,
    packed::CellOutput,
    prelude::*,
    utilities::DIFF_TWO,
    U256,
};
use tempfile::TempDir;

use crate::DaoCalculator;

fn prepare_store(
    parent: &HeaderView,
    epoch_start: Option<BlockNumber>,
) -> (TempDir, ChainDB, HeaderView) {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let store = ChainDB::new(db, Default::default());
    let txn = store.begin_transaction();

    let parent_block = BlockBuilder::default().header(parent.clone()).build();

    txn.insert_block(&parent_block).unwrap();
    txn.attach_block(&parent_block).unwrap();

    let epoch_ext = EpochExt::new_builder()
        .number(parent.number())
        .base_block_reward(Capacity::shannons(50_000_000_000))
        .remainder_reward(Capacity::shannons(1_000_128))
        .previous_epoch_hash_rate(U256::one())
        .last_block_hash_in_previous_epoch(h256!("0x1").pack())
        .start_number(epoch_start.unwrap_or_else(|| parent.number() - 1000))
        .length(2091)
        .compact_target(DIFF_TWO)
        .build();
    let epoch_hash = h256!("0x123455").pack();

    txn.insert_block_epoch_index(&parent.hash(), &epoch_hash)
        .unwrap();
    txn.insert_epoch_ext(&epoch_hash, &epoch_ext).unwrap();

    txn.commit().unwrap();

    (tmp_dir, store, parent.clone())
}

#[test]
fn check_dao_data_calculation() {
    let consensus = Consensus::default();

    let parent_number = 12345;
    let parent_header = HeaderBuilder::default()
        .number(parent_number.pack())
        .dao(pack_dao_data(
            10_000_000_000_123_456,
            Capacity::shannons(500_000_000_123_000),
            Capacity::shannons(400_000_000_123),
            Capacity::shannons(600_000_000_000),
        ))
        .build();

    let (_tmp_dir, store, parent_header) = prepare_store(&parent_header, None);
    let result = DaoCalculator::new(&consensus, &store.as_data_provider())
        .dao_field(&[], &parent_header)
        .unwrap();
    let dao_data = extract_dao_data(result);
    assert_eq!(
        dao_data,
        (
            10_000_586_990_682_998,
            Capacity::shannons(500_079_349_650_985),
            Capacity::shannons(429_314_308_674),
            Capacity::shannons(600_000_000_000)
        )
    );
}

#[test]
fn check_initial_dao_data_calculation() {
    let consensus = Consensus::default();

    let parent_number = 0;
    let parent_header = HeaderBuilder::default()
        .number(parent_number.pack())
        .dao(pack_dao_data(
            10_000_000_000_000_000,
            Capacity::shannons(500_000_000_000_000),
            Capacity::shannons(400_000_000_000),
            Capacity::shannons(600_000_000_000),
        ))
        .build();

    let (_tmp_dir, store, parent_header) = prepare_store(&parent_header, Some(0));
    let result = DaoCalculator::new(&consensus, &store.as_data_provider())
        .dao_field(&[], &parent_header)
        .unwrap();
    let dao_data = extract_dao_data(result);
    assert_eq!(
        dao_data,
        (
            10_000_586_990_559_680,
            Capacity::shannons(500_079_349_527_985),
            Capacity::shannons(429_314_308_551),
            Capacity::shannons(600_000_000_000)
        )
    );
}

#[test]
fn check_first_epoch_block_dao_data_calculation() {
    let consensus = Consensus::default();

    let parent_number = 12340;
    let parent_header = HeaderBuilder::default()
        .number(parent_number.pack())
        .dao(pack_dao_data(
            10_000_000_000_123_456,
            Capacity::shannons(500_000_000_123_000),
            Capacity::shannons(400_000_000_123),
            Capacity::shannons(600_000_000_000),
        ))
        .build();

    let (_tmp_dir, store, parent_header) = prepare_store(&parent_header, Some(12340));
    let result = DaoCalculator::new(&consensus, &store.as_data_provider())
        .dao_field(&[], &parent_header)
        .unwrap();
    let dao_data = extract_dao_data(result);
    assert_eq!(
        dao_data,
        (
            10_000_586_990_682_998,
            Capacity::shannons(500_079_349_650_985),
            Capacity::shannons(429_314_308_674),
            Capacity::shannons(600_000_000_000)
        )
    );
}

#[test]
fn check_dao_data_calculation_overflows() {
    let consensus = Consensus::default();

    let parent_number = 12345;
    let parent_header = HeaderBuilder::default()
        .number(parent_number.pack())
        .dao(pack_dao_data(
            10_000_000_000_123_456,
            Capacity::shannons(18_446_744_073_709_000_000),
            Capacity::shannons(446_744_073_709),
            Capacity::shannons(600_000_000_000),
        ))
        .build();

    let (_tmp_dir, store, parent_header) = prepare_store(&parent_header, None);
    let result =
        DaoCalculator::new(&consensus, &store.as_data_provider()).dao_field(&[], &parent_header);
    assert!(result.unwrap_err().to_string().contains("Overflow"));
}

#[test]
fn check_dao_data_calculation_with_transactions() {
    let consensus = Consensus::default();

    let parent_number = 12345;
    let parent_header = HeaderBuilder::default()
        .number(parent_number.pack())
        .dao(pack_dao_data(
            10_000_000_000_123_456,
            Capacity::shannons(500_000_000_123_000),
            Capacity::shannons(400_000_000_123),
            Capacity::shannons(600_000_000_000),
        ))
        .build();

    let (_tmp_dir, store, parent_header) = prepare_store(&parent_header, None);
    let input_cell_data = Bytes::from("abcde");
    let input_cell = CellOutput::new_builder()
        .capacity(capacity_bytes!(10000).pack())
        .build();
    let output_cell_data = Bytes::from("abcde12345");
    let output_cell = CellOutput::new_builder()
        .capacity(capacity_bytes!(20000).pack())
        .build();

    let tx = TransactionBuilder::default()
        .output(output_cell)
        .output_data(output_cell_data.pack())
        .build();
    let rtx = ResolvedTransaction {
        transaction: tx,
        resolved_cell_deps: vec![],
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(input_cell, input_cell_data).build(),
        ],
        resolved_dep_groups: vec![],
    };

    let result = DaoCalculator::new(&consensus, &store.as_data_provider())
        .dao_field(&[rtx], &parent_header)
        .unwrap();
    let dao_data = extract_dao_data(result);
    assert_eq!(
        dao_data,
        (
            10_000_586_990_682_998,
            Capacity::shannons(500_079_349_650_985),
            Capacity::shannons(429_314_308_674),
            Capacity::shannons(600_500_000_000)
        )
    );
}

#[test]
fn check_withdraw_calculation() {
    let data = Bytes::from(vec![1; 10]);
    let output = CellOutput::new_builder()
        .capacity(capacity_bytes!(1000000).pack())
        .build();
    let tx = TransactionBuilder::default()
        .output(output.clone())
        .output_data(data.pack())
        .build();
    let deposit_header = HeaderBuilder::default()
        .number(100.pack())
        .dao(pack_dao_data(
            10_000_000_000_123_456,
            Default::default(),
            Default::default(),
            Default::default(),
        ))
        .build();
    let deposit_block = BlockBuilder::default()
        .header(deposit_header)
        .transaction(tx)
        .build();

    let withdrawing_header = HeaderBuilder::default()
        .number(200.pack())
        .dao(pack_dao_data(
            10_000_000_001_123_456,
            Default::default(),
            Default::default(),
            Default::default(),
        ))
        .build();
    let withdrawing_block = BlockBuilder::default().header(withdrawing_header).build();

    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let store = ChainDB::new(db, Default::default());
    let txn = store.begin_transaction();
    txn.insert_block(&deposit_block).unwrap();
    txn.attach_block(&deposit_block).unwrap();
    txn.insert_block(&withdrawing_block).unwrap();
    txn.attach_block(&withdrawing_block).unwrap();
    txn.commit().unwrap();

    let consensus = Consensus::default();
    let data_loader = store.as_data_provider();
    let calculator = DaoCalculator::new(&consensus, &data_loader);
    let result = calculator.calculate_maximum_withdraw(
        &output,
        Capacity::bytes(data.len()).expect("should not overlfow"),
        &deposit_block.hash(),
        &withdrawing_block.hash(),
    );
    assert_eq!(result.unwrap(), Capacity::shannons(100_000_000_009_999));
}

#[test]
fn check_withdraw_calculation_overflows() {
    let output = CellOutput::new_builder()
        .capacity(Capacity::shannons(18_446_744_073_709_550_000).pack())
        .build();
    let tx = TransactionBuilder::default().output(output.clone()).build();
    let deposit_header = HeaderBuilder::default()
        .number(100.pack())
        .dao(pack_dao_data(
            10_000_000_000_123_456,
            Default::default(),
            Default::default(),
            Default::default(),
        ))
        .build();
    let deposit_block = BlockBuilder::default()
        .header(deposit_header)
        .transaction(tx)
        .build();

    let withdrawing_header = HeaderBuilder::default()
        .number(200.pack())
        .dao(pack_dao_data(
            10_000_000_001_123_456,
            Default::default(),
            Default::default(),
            Default::default(),
        ))
        .build();
    let withdrawing_block = BlockBuilder::default().header(withdrawing_header).build();

    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let store = ChainDB::new(db, Default::default());
    let txn = store.begin_transaction();
    txn.insert_block(&deposit_block).unwrap();
    txn.attach_block(&deposit_block).unwrap();
    txn.insert_block(&withdrawing_block).unwrap();
    txn.attach_block(&withdrawing_block).unwrap();
    txn.commit().unwrap();

    let consensus = Consensus::default();
    let data_loader = store.as_data_provider();
    let calculator = DaoCalculator::new(&consensus, &data_loader);
    let result = calculator.calculate_maximum_withdraw(
        &output,
        Capacity::bytes(0).expect("should not overlfow"),
        &deposit_block.hash(),
        &withdrawing_block.hash(),
    );
    assert!(result.is_err());
}
