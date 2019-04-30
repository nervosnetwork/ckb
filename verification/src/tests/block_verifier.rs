use super::super::block_verifier::{BlockVerifier, CellbaseVerifier};
use super::super::error::{CellbaseError, Error as VerifyError};
use super::dummy::DummyChainProvider;
use crate::Verifier;
use ckb_core::block::BlockBuilder;
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, Transaction, TransactionBuilder};
use ckb_core::{capacity_bytes, Bytes, Capacity};
use numext_fixed_hash::H256;

fn create_cellbase_transaction_with_capacity(capacity: Capacity) -> Transaction {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .output(CellOutput::new(
            capacity,
            Bytes::default(),
            Script::default(),
            None,
        ))
        .build()
}

fn create_cellbase_transaction() -> Transaction {
    create_cellbase_transaction_with_capacity(capacity_bytes!(100))
}

fn create_normal_transaction() -> Transaction {
    TransactionBuilder::default()
        .input(CellInput::new(
            OutPoint::new(H256::from_trimmed_hex_str("1").unwrap(), 0),
            0,
            Default::default(),
        ))
        .output(CellOutput::new(
            capacity_bytes!(100),
            Bytes::default(),
            Script::default(),
            None,
        ))
        .build()
}

#[test]
pub fn test_block_without_cellbase() {
    let block = BlockBuilder::default()
        .transaction(TransactionBuilder::default().build())
        .build();
    let verifier = CellbaseVerifier::new();
    assert_eq!(
        verifier.verify(&block),
        Err(VerifyError::Cellbase(CellbaseError::InvalidQuantity))
    );
}

#[test]
pub fn test_block_with_one_cellbase_at_first() {
    let transaction = create_normal_transaction();

    let block = BlockBuilder::default()
        .transaction(create_cellbase_transaction())
        .transaction(transaction)
        .build();

    let verifier = CellbaseVerifier::new();
    assert!(verifier.verify(&block).is_ok());
}

#[test]
pub fn test_block_with_one_cellbase_at_last() {
    let block = BlockBuilder::default()
        .transaction(create_normal_transaction())
        .transaction(create_cellbase_transaction())
        .build();

    let verifier = CellbaseVerifier::new();
    assert_eq!(
        verifier.verify(&block),
        Err(VerifyError::Cellbase(CellbaseError::InvalidPosition))
    );
}

#[test]
pub fn test_block_with_two_cellbases() {
    let block = BlockBuilder::default()
        .transaction(create_cellbase_transaction())
        .transaction(create_cellbase_transaction())
        .build();

    let verifier = CellbaseVerifier::new();
    assert_eq!(
        verifier.verify(&block),
        Err(VerifyError::Cellbase(CellbaseError::InvalidQuantity))
    );
}

#[test]
pub fn test_cellbase_with_less_reward() {
    let transaction = create_normal_transaction();

    let block = BlockBuilder::default()
        .transaction(create_cellbase_transaction_with_capacity(capacity_bytes!(
            50
        )))
        .transaction(transaction)
        .build();

    let verifier = CellbaseVerifier::new();
    assert!(verifier.verify(&block).is_ok());
}

#[test]
pub fn test_cellbase_with_fee() {
    let transaction = create_normal_transaction();

    let block = BlockBuilder::default()
        .transaction(create_cellbase_transaction_with_capacity(capacity_bytes!(
            110
        )))
        .transaction(transaction)
        .build();

    let verifier = CellbaseVerifier::new();
    assert!(verifier.verify(&block).is_ok());
}

#[test]
pub fn test_cellbase_overflow_capacity() {
    let cellbase = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .output(CellOutput::new(
            capacity_bytes!(5),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9].into(),
            Script::default(),
            None,
        ))
        .build();
    let block = BlockBuilder::default().transaction(cellbase).build();
    let verifier = CellbaseVerifier::new();
    assert_eq!(verifier.verify(&block), Err(VerifyError::CapacityOverflow),);
}

#[test]
pub fn test_empty_transactions() {
    let block = BlockBuilder::default().build();

    let provider = DummyChainProvider {
        block_reward: capacity_bytes!(150),
        ..Default::default()
    };

    let full_verifier = BlockVerifier::new(provider);
    // short-circuit, Empty check first
    assert_eq!(
        full_verifier.verify(&block),
        Err(VerifyError::Cellbase(CellbaseError::InvalidQuantity))
    );
}
