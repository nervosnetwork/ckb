use super::super::transaction_verifier::CellbaseVerifier;
use super::utils::create_dummy_transaction;
use bigint::H256;
use core::transaction::{CellInput, CellOutput, OutPoint};

#[test]
pub fn test_cellbase_with_one_output() {
    let mut transaction = create_dummy_transaction();
    transaction
        .inputs
        .push(CellInput::new(OutPoint::null(), Default::default()));
    transaction
        .outputs
        .push(CellOutput::new(0, 100, Vec::new(), H256::default()));

    let verifier = CellbaseVerifier::new(&transaction);
    assert!(verifier.verify().is_ok());
}

#[test]
pub fn test_cellbase_with_two_outputs() {
    let mut transaction = create_dummy_transaction();
    transaction
        .inputs
        .push(CellInput::new(OutPoint::null(), Default::default()));
    transaction
        .outputs
        .push(CellOutput::new(0, 100, Vec::new(), H256::default()));
    transaction
        .outputs
        .push(CellOutput::new(0, 100, Vec::new(), H256::default()));

    let verifier = CellbaseVerifier::new(&transaction);
    assert!(verifier.verify().is_err());
}

#[test]
pub fn test_non_cellbase_with_one_output() {
    let mut transaction = create_dummy_transaction();
    transaction.inputs.push(CellInput::new(
        OutPoint::new(H256::from(1), 0),
        Default::default(),
    ));
    transaction
        .outputs
        .push(CellOutput::new(0, 100, Vec::new(), H256::default()));

    let verifier = CellbaseVerifier::new(&transaction);
    assert!(verifier.verify().is_ok());
}

#[test]
pub fn test_non_cellbase_with_two_outputs() {
    let mut transaction = create_dummy_transaction();
    transaction.inputs.push(CellInput::new(
        OutPoint::new(H256::from(1), 0),
        Default::default(),
    ));
    transaction
        .outputs
        .push(CellOutput::new(0, 100, Vec::new(), H256::default()));
    transaction
        .outputs
        .push(CellOutput::new(0, 100, Vec::new(), H256::default()));

    let verifier = CellbaseVerifier::new(&transaction);
    assert!(verifier.verify().is_ok());
}
