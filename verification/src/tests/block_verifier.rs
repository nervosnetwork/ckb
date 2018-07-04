use super::super::block_verifier::CellbaseTransactionsVerifier;
use super::utils::{create_dummy_block, create_dummy_transaction};
use bigint::H256;
use core::transaction::{CellInput, CellOutput, OutPoint, Transaction};

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

    let verifier = CellbaseTransactionsVerifier::new(&block);
    assert!(verifier.verify().is_ok());
}

#[test]
pub fn test_block_with_one_cellbase_at_first() {
    let mut block = create_dummy_block();
    block.transactions.push(create_cellbase_transaction());
    block.transactions.push(create_normal_transaction());

    let verifier = CellbaseTransactionsVerifier::new(&block);
    assert!(verifier.verify().is_ok());
}

#[test]
pub fn test_block_with_one_cellbase_at_last() {
    let mut block = create_dummy_block();
    block.transactions.push(create_normal_transaction());
    block.transactions.push(create_cellbase_transaction());

    let verifier = CellbaseTransactionsVerifier::new(&block);
    assert!(verifier.verify().is_err());
}

#[test]
pub fn test_block_with_two_cellbases() {
    let mut block = create_dummy_block();
    block.transactions.push(create_cellbase_transaction());
    block.transactions.push(create_normal_transaction());
    block.transactions.push(create_cellbase_transaction());

    let verifier = CellbaseTransactionsVerifier::new(&block);
    assert!(verifier.verify().is_err());
}
