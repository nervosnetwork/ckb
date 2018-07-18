use super::super::block_verifier::CellbaseTransactionsVerifier;
use super::dummy::DummyChainClient;
use super::utils::{create_dummy_block, create_dummy_transaction};
use bigint::H256;
use chain::chain::{ChainProvider, Error};
use core::transaction::{CellInput, CellOutput, OutPoint, Transaction};
use std::collections::HashMap;
use std::sync::Arc;

fn create_cellbase_transaction_with_capacity(capacity: u32) -> Transaction {
    let mut transaction = create_dummy_transaction();
    transaction
        .inputs
        .push(CellInput::new(OutPoint::null(), Default::default()));
    transaction
        .outputs
        .push(CellOutput::new(0, capacity, Vec::new(), H256::default()));

    transaction
}

fn create_cellbase_transaction() -> Transaction {
    create_cellbase_transaction_with_capacity(100)
}

fn create_normal_transaction() -> Transaction {
    let mut transaction = create_dummy_transaction();
    transaction.inputs.push(CellInput::new(
        OutPoint::new(H256::from(1), 0),
        Default::default(),
    ));
    transaction
        .outputs
        .push(CellOutput::new(0, 100, Vec::new(), H256::default()));

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
    let mut transaction_fees = HashMap::<H256, Result<u32, Error>>::new();

    block.transactions.push(create_cellbase_transaction());

    let transaction = create_normal_transaction();
    transaction_fees.insert(transaction.hash(), Ok(0));
    block.transactions.push(create_normal_transaction());

    let chain = Arc::new(DummyChainClient {
        block_reward: 100,
        transaction_fees: transaction_fees,
    });

    let verifier = CellbaseTransactionsVerifier::new(&block, chain);
    assert!(verifier.verify().is_ok());
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

#[test]
pub fn test_cellbase_with_less_reward() {
    let mut block = create_dummy_block();
    let mut transaction_fees = HashMap::<H256, Result<u32, Error>>::new();

    block
        .transactions
        .push(create_cellbase_transaction_with_capacity(50));

    let transaction = create_normal_transaction();
    transaction_fees.insert(transaction.hash(), Ok(0));
    block.transactions.push(create_normal_transaction());

    let chain = Arc::new(DummyChainClient {
        block_reward: 100,
        transaction_fees: transaction_fees,
    });

    let verifier = CellbaseTransactionsVerifier::new(&block, chain);
    assert!(verifier.verify().is_ok());
}

#[test]
pub fn test_cellbase_with_fee() {
    let mut block = create_dummy_block();
    let mut transaction_fees = HashMap::<H256, Result<u32, Error>>::new();

    block
        .transactions
        .push(create_cellbase_transaction_with_capacity(110));

    let transaction = create_normal_transaction();
    transaction_fees.insert(transaction.hash(), Ok(10));
    block.transactions.push(create_normal_transaction());

    let chain = Arc::new(DummyChainClient {
        block_reward: 100,
        transaction_fees: transaction_fees,
    });

    let verifier = CellbaseTransactionsVerifier::new(&block, chain);
    assert!(verifier.verify().is_ok());
}

#[test]
pub fn test_cellbase_with_more_reward_than_available() {
    let mut block = create_dummy_block();
    let mut transaction_fees = HashMap::<H256, Result<u32, Error>>::new();

    block
        .transactions
        .push(create_cellbase_transaction_with_capacity(130));

    let transaction = create_normal_transaction();
    transaction_fees.insert(transaction.hash(), Ok(10));
    block.transactions.push(create_normal_transaction());

    let chain = Arc::new(DummyChainClient {
        block_reward: 100,
        transaction_fees: transaction_fees,
    });

    let verifier = CellbaseTransactionsVerifier::new(&block, chain);
    assert!(verifier.verify().is_err());
}

#[test]
pub fn test_cellbase_with_invalid_transaction() {
    let mut block = create_dummy_block();
    let mut transaction_fees = HashMap::<H256, Result<u32, Error>>::new();

    block
        .transactions
        .push(create_cellbase_transaction_with_capacity(100));

    let transaction = create_normal_transaction();
    transaction_fees.insert(transaction.hash(), Err(Error::InvalidOutput));
    block.transactions.push(create_normal_transaction());

    let chain = Arc::new(DummyChainClient {
        block_reward: 100,
        transaction_fees: transaction_fees,
    });

    let verifier = CellbaseTransactionsVerifier::new(&block, chain);
    assert!(verifier.verify().is_err());
}

#[test]
pub fn test_cellbase_with_two_outputs() {
    let mut block = create_dummy_block();
    let mut transaction_fees = HashMap::<H256, Result<u32, Error>>::new();

    let mut cellbase_transaction = create_cellbase_transaction_with_capacity(100);
    // Add another output
    cellbase_transaction
        .outputs
        .push(CellOutput::new(0, 50, Vec::new(), H256::default()));
    block.transactions.push(cellbase_transaction);

    let transaction = create_normal_transaction();
    transaction_fees.insert(transaction.hash(), Ok(0));
    block.transactions.push(create_normal_transaction());

    let chain = Arc::new(DummyChainClient {
        block_reward: 150,
        transaction_fees: transaction_fees,
    });

    let verifier = CellbaseTransactionsVerifier::new(&block, chain);
    assert!(verifier.verify().is_ok());
}

#[test]
pub fn test_cellbase_with_two_outputs_and_more_rewards_than_maximum() {
    let mut block = create_dummy_block();
    let mut transaction_fees = HashMap::<H256, Result<u32, Error>>::new();

    let mut cellbase_transaction = create_cellbase_transaction_with_capacity(100);
    // Add another output
    cellbase_transaction
        .outputs
        .push(CellOutput::new(0, 50, Vec::new(), H256::default()));
    block.transactions.push(cellbase_transaction);

    let transaction = create_normal_transaction();
    transaction_fees.insert(transaction.hash(), Ok(0));
    block.transactions.push(create_normal_transaction());

    let chain = Arc::new(DummyChainClient {
        block_reward: 100,
        transaction_fees: transaction_fees,
    });

    let verifier = CellbaseTransactionsVerifier::new(&block, chain);
    assert!(verifier.verify().is_err());
}

fn dummy_chain() -> Arc<impl ChainProvider> {
    Arc::new(DummyChainClient {
        block_reward: 0,
        transaction_fees: HashMap::new(),
    })
}
