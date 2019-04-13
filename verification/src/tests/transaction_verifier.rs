use super::super::transaction_verifier::{
    CapacityVerifier, DuplicateInputsVerifier, EmptyVerifier, NullVerifier,
};
use crate::error::TransactionError;
use ckb_core::cell::ResolvedTransaction;
use ckb_core::cell::{CellStatus, LiveCell};
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, TransactionBuilder};
use numext_fixed_hash::H256;

#[test]
pub fn test_null() {
    let transaction = TransactionBuilder::default()
        .input(CellInput::new(
            OutPoint::new(H256::zero(), u32::max_value()),
            Default::default(),
        ))
        .build();
    let verifier = NullVerifier::new(&transaction);
    assert_eq!(verifier.verify().err(), Some(TransactionError::NullInput));
}

#[test]
pub fn test_empty() {
    let transaction = TransactionBuilder::default().build();
    let verifier = EmptyVerifier::new(&transaction);

    assert_eq!(verifier.verify().err(), Some(TransactionError::Empty));
}

#[test]
pub fn test_capacity_outofbound() {
    let transaction = TransactionBuilder::default()
        .output(CellOutput::new(50, vec![1; 51], Script::default(), None))
        .build();

    let rtx = ResolvedTransaction {
        transaction,
        dep_cells: Vec::new(),
        input_cells: vec![CellStatus::Live(LiveCell::Output(CellOutput::new(
            50,
            Vec::new(),
            Script::default(),
            None,
        )))],
    };
    let verifier = CapacityVerifier::new(&rtx);

    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::CapacityOverflow)
    );
}

#[test]
pub fn test_capacity_invalid() {
    let transaction = TransactionBuilder::default()
        .outputs(vec![
            CellOutput::new(50, Vec::new(), Script::default(), None),
            CellOutput::new(100, Vec::new(), Script::default(), None),
        ])
        .build();

    let rtx = ResolvedTransaction {
        transaction,
        dep_cells: Vec::new(),
        input_cells: vec![
            CellStatus::Live(LiveCell::Output(CellOutput::new(
                49,
                Vec::new(),
                Script::default(),
                None,
            ))),
            CellStatus::Live(LiveCell::Output(CellOutput::new(
                100,
                Vec::new(),
                Script::default(),
                None,
            ))),
        ],
    };
    let verifier = CapacityVerifier::new(&rtx);

    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::OutputsSumOverflow)
    );
}

#[test]
pub fn test_duplicate_inputs() {
    let transaction = TransactionBuilder::default()
        .inputs(vec![
            CellInput::new(
                OutPoint::new(H256::from_trimmed_hex_str("1").unwrap(), 0),
                Default::default(),
            ),
            CellInput::new(
                OutPoint::new(H256::from_trimmed_hex_str("1").unwrap(), 0),
                Default::default(),
            ),
        ])
        .build();

    let verifier = DuplicateInputsVerifier::new(&transaction);

    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::DuplicateInputs)
    );
}
