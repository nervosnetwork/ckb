use super::super::block_verifier::{BlockVerifier, CellbaseVerifier};
use super::super::error::{CellbaseError, Error as VerifyError};
use super::dummy::DummyChainProvider;
use crate::Verifier;
use ckb_core::block::BlockBuilder;
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, Transaction, TransactionBuilder};
use ckb_core::Capacity;
use ckb_shared::error::SharedError;
use numext_fixed_hash::H256;
use std::collections::HashMap;

fn create_cellbase_transaction_with_capacity(capacity: Capacity) -> Transaction {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .output(CellOutput::new(
            capacity,
            Vec::new(),
            Script::default(),
            None,
        ))
        .build()
}

fn create_cellbase_transaction() -> Transaction {
    create_cellbase_transaction_with_capacity(100)
}

fn create_normal_transaction() -> Transaction {
    TransactionBuilder::default()
        .input(CellInput::new(
            OutPoint::new(H256::from_trimmed_hex_str("1").unwrap(), 0),
            0,
            Default::default(),
        ))
        .output(CellOutput::new(100, Vec::new(), Script::default(), None))
        .build()
}

#[test]
pub fn test_block_without_cellbase() {
    let block = BlockBuilder::default()
        .commit_transaction(TransactionBuilder::default().build())
        .build();
    let verifier = CellbaseVerifier::new(DummyChainProvider::default());
    assert_eq!(
        verifier.verify(&block),
        Err(VerifyError::Cellbase(CellbaseError::InvalidQuantity))
    );
}

#[test]
pub fn test_block_with_one_cellbase_at_first() {
    let mut transaction_fees = HashMap::<H256, Result<Capacity, SharedError>>::new();
    let transaction = create_normal_transaction();
    transaction_fees.insert(transaction.hash().clone(), Ok(0));

    let block = BlockBuilder::default()
        .commit_transaction(create_cellbase_transaction())
        .commit_transaction(transaction)
        .build();

    let provider = DummyChainProvider {
        block_reward: 100,
        transaction_fees,
    };

    let verifier = CellbaseVerifier::new(provider);
    assert!(verifier.verify(&block).is_ok());
}

#[test]
pub fn test_block_with_one_cellbase_at_last() {
    let block = BlockBuilder::default()
        .commit_transaction(create_normal_transaction())
        .commit_transaction(create_cellbase_transaction())
        .build();

    let verifier = CellbaseVerifier::new(DummyChainProvider::default());
    assert_eq!(
        verifier.verify(&block),
        Err(VerifyError::Cellbase(CellbaseError::InvalidPosition))
    );
}

#[test]
pub fn test_block_with_two_cellbases() {
    let block = BlockBuilder::default()
        .commit_transaction(create_cellbase_transaction())
        .commit_transaction(create_cellbase_transaction())
        .build();

    let verifier = CellbaseVerifier::new(DummyChainProvider::default());
    assert_eq!(
        verifier.verify(&block),
        Err(VerifyError::Cellbase(CellbaseError::InvalidQuantity))
    );
}

#[test]
pub fn test_cellbase_with_less_reward() {
    let mut transaction_fees = HashMap::<H256, Result<Capacity, SharedError>>::new();
    let transaction = create_normal_transaction();
    transaction_fees.insert(transaction.hash().clone(), Ok(0));

    let block = BlockBuilder::default()
        .commit_transaction(create_cellbase_transaction_with_capacity(50))
        .commit_transaction(transaction)
        .build();

    let provider = DummyChainProvider {
        block_reward: 100,
        transaction_fees,
    };

    let verifier = CellbaseVerifier::new(provider);
    assert!(verifier.verify(&block).is_ok());
}

#[test]
pub fn test_cellbase_with_fee() {
    let mut transaction_fees = HashMap::<H256, Result<Capacity, SharedError>>::new();
    let transaction = create_normal_transaction();
    transaction_fees.insert(transaction.hash().clone(), Ok(10));

    let block = BlockBuilder::default()
        .commit_transaction(create_cellbase_transaction_with_capacity(110))
        .commit_transaction(transaction)
        .build();

    let provider = DummyChainProvider {
        block_reward: 100,
        transaction_fees,
    };

    let verifier = CellbaseVerifier::new(provider);
    assert!(verifier.verify(&block).is_ok());
}

#[test]
pub fn test_empty_transactions() {
    let block = BlockBuilder::default().build();
    let transaction_fees = HashMap::<H256, Result<Capacity, SharedError>>::new();

    let provider = DummyChainProvider {
        block_reward: 150,
        transaction_fees,
    };

    let full_verifier = BlockVerifier::new(provider);
    // short-circuit, Empty check first
    assert_eq!(
        full_verifier.verify(&block),
        Err(VerifyError::Cellbase(CellbaseError::InvalidQuantity))
    );
}
