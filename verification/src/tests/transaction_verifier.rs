use super::super::error::TransactionError;
use super::super::transaction_verifier::{
    CapacityVerifier, DuplicateInputsVerifier, EmptyVerifier, NullVerifier,
};
use super::dummy::DummyCellState;
use bigint::H256;
use core::cell::ResolvedTransaction;
use core::transaction::{CellInput, CellOutput, OutPoint, Transaction};

#[test]
pub fn test_null() {
    let transaction = Transaction::new(
        0,
        Vec::new(),
        vec![CellInput::new(
            OutPoint::new(H256::from(0), u32::max_value()),
            Default::default(),
        )],
        Vec::new(),
    );
    let verifier = NullVerifier::new(&transaction);

    assert_eq!(verifier.verify().err(), Some(TransactionError::NullInput));
}

#[test]
pub fn test_empty() {
    let transaction = Transaction::new(0, Vec::new(), Vec::new(), Vec::new());
    let verifier = EmptyVerifier::new(&transaction);

    assert_eq!(verifier.verify().err(), Some(TransactionError::Empty));
}

#[test]
pub fn test_capacity_outofbound() {
    let transaction = Transaction::new(
        0,
        Vec::new(),
        Vec::new(),
        vec![CellOutput::new(50, vec![1; 51], H256::from(0))],
    );
    let rtx = ResolvedTransaction {
        transaction,
        dep_cells: Vec::new(),
        input_cells: vec![DummyCellState::Head(CellOutput::new(
            50,
            Vec::new(),
            H256::from(0),
        ))],
    };
    let verifier = CapacityVerifier::new(&rtx);

    assert_eq!(verifier.verify().err(), Some(TransactionError::OutofBound));
}

#[test]
pub fn test_capacity_invalid() {
    let transaction = Transaction::new(
        0,
        Vec::new(),
        Vec::new(),
        vec![
            CellOutput::new(50, Vec::new(), H256::from(0)),
            CellOutput::new(100, Vec::new(), H256::from(0)),
        ],
    );
    let rtx = ResolvedTransaction {
        transaction,
        dep_cells: Vec::new(),
        input_cells: vec![
            DummyCellState::Head(CellOutput::new(49, Vec::new(), H256::from(0))),
            DummyCellState::Head(CellOutput::new(100, Vec::new(), H256::from(0))),
        ],
    };
    let verifier = CapacityVerifier::new(&rtx);

    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::InvalidCapacity)
    );
}

#[test]
pub fn test_duplicate_inputs() {
    let transaction = Transaction::new(
        0,
        Vec::new(),
        vec![
            CellInput::new(OutPoint::new(H256::from(1), 0), Default::default()),
            CellInput::new(OutPoint::new(H256::from(1), 0), Default::default()),
        ],
        Vec::new(),
    );
    let verifier = DuplicateInputsVerifier::new(&transaction);

    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::DuplicateInputs)
    );
}
