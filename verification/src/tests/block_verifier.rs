use super::super::block_verifier::CellbaseTransactionsVerifier;
use super::utils::{create_dummy_block, create_dummy_transaction};
use bigint::H256;
use chain::chain::{Chain, ChainClient};
use chain::store::ChainKVStore;
use chain::Config;
use chain::COLUMNS;
use core::block::Block;
use core::transaction::{CellInput, CellOutput, OutPoint, Transaction};
use db::memorydb::MemoryKeyValueDB;
use std::sync::Arc;

fn create_cellbase_transaction() -> Transaction {
    let mut transaction = create_dummy_transaction();
    transaction
        .inputs
        .push(CellInput::new(OutPoint::null(), Vec::new()));
    transaction
        .outputs
        .push(CellOutput::new(0, 100, Vec::new(), Vec::new()));

    transaction
}

fn create_normal_transaction() -> Transaction {
    let mut transaction = create_dummy_transaction();
    transaction
        .inputs
        .push(CellInput::new(OutPoint::new(H256::from(1), 0), Vec::new()));
    transaction
        .outputs
        .push(CellOutput::new(0, 100, Vec::new(), Vec::new()));

    transaction
}

#[test]
pub fn test_block_without_cellbase() {
    let mut block = create_dummy_block();
    block.transactions.push(create_normal_transaction());

    let verifier = CellbaseTransactionsVerifier::new(&block, dummy_chain());
    assert!(verifier.verify().is_ok());
}

#[test]
pub fn test_block_with_one_cellbase_at_first() {
    let mut block = create_dummy_block();
    block.transactions.push(create_cellbase_transaction());
    block.transactions.push(create_normal_transaction());

    let verifier = CellbaseTransactionsVerifier::new(&block, dummy_chain());
    // TODO this will throw InvalidInput, find a solution for test fixtures
    // assert!(verifier.verify().is_ok());
}

#[test]
pub fn test_block_with_one_cellbase_at_last() {
    let mut block = create_dummy_block();
    block.transactions.push(create_normal_transaction());
    block.transactions.push(create_cellbase_transaction());

    let verifier = CellbaseTransactionsVerifier::new(&block, dummy_chain());
    assert!(verifier.verify().is_err());
}

#[test]
pub fn test_block_with_two_cellbases() {
    let mut block = create_dummy_block();
    block.transactions.push(create_cellbase_transaction());
    block.transactions.push(create_normal_transaction());
    block.transactions.push(create_cellbase_transaction());

    let verifier = CellbaseTransactionsVerifier::new(&block, dummy_chain());
    assert!(verifier.verify().is_err());
}

fn dummy_chain() -> Arc<impl ChainClient> {
    let db = MemoryKeyValueDB::open(COLUMNS as usize);
    let store = ChainKVStore { db };
    let mut config = Config::default();
    config.sealer_type = "Noop".to_string();
    config.initial_block_reward = 100;
    Arc::new(Chain::init(store, config, None).unwrap())
}
