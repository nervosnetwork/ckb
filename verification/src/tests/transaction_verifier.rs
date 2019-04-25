use super::super::transaction_verifier::{
    CapacityVerifier, DuplicateInputsVerifier, EmptyVerifier, MaturityVerifier, NullVerifier,
    ValidSinceVerifier,
};
use crate::error::TransactionError;
use ckb_core::cell::CellStatus;
use ckb_core::cell::ResolvedTransaction;
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, TransactionBuilder};
use ckb_core::{capacity_bytes, Capacity};
use ckb_traits::BlockMedianTimeContext;
use numext_fixed_hash::H256;

#[test]
pub fn test_null() {
    let transaction = TransactionBuilder::default()
        .input(CellInput::new(
            OutPoint::new(H256::zero(), u32::max_value()),
            0,
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
        .output(CellOutput::new(
            capacity_bytes!(50),
            vec![1; 51],
            Script::default(),
            None,
        ))
        .build();

    let rtx = ResolvedTransaction {
        transaction,
        dep_cells: Vec::new(),
        input_cells: vec![CellStatus::live_output(
            CellOutput::new(capacity_bytes!(50), Vec::new(), Script::default(), None),
            None,
            false,
        )],
    };
    let verifier = CapacityVerifier::new(&rtx);

    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::CapacityOverflow)
    );
}

#[test]
pub fn test_cellbase_maturity() {
    let transaction = TransactionBuilder::default()
        .output(CellOutput::new(
            capacity_bytes!(50),
            vec![1; 51],
            Script::default(),
            None,
        ))
        .build();

    let rtx = ResolvedTransaction {
        transaction,
        dep_cells: Vec::new(),
        input_cells: vec![CellStatus::live_output(
            CellOutput::new(capacity_bytes!(50), Vec::new(), Script::default(), None),
            Some(30),
            true,
        )],
    };

    let tip_number = 70;
    let cellbase_maturity = 100;
    let verifier = MaturityVerifier::new(&rtx, tip_number, cellbase_maturity);

    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::CellbaseImmaturity)
    );

    let tip_number = 130;
    let verifier = MaturityVerifier::new(&rtx, tip_number, cellbase_maturity);

    assert!(verifier.verify().is_ok());
}

#[test]
pub fn test_capacity_invalid() {
    let transaction = TransactionBuilder::default()
        .outputs(vec![
            CellOutput::new(capacity_bytes!(50), Vec::new(), Script::default(), None),
            CellOutput::new(capacity_bytes!(100), Vec::new(), Script::default(), None),
        ])
        .build();

    let rtx = ResolvedTransaction {
        transaction,
        dep_cells: Vec::new(),
        input_cells: vec![
            CellStatus::live_output(
                CellOutput::new(capacity_bytes!(49), Vec::new(), Script::default(), None),
                None,
                false,
            ),
            CellStatus::live_output(
                CellOutput::new(capacity_bytes!(100), Vec::new(), Script::default(), None),
                None,
                false,
            ),
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
                0,
                Default::default(),
            ),
            CellInput::new(
                OutPoint::new(H256::from_trimmed_hex_str("1").unwrap(), 0),
                0,
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

struct FakeMedianTime {
    timestamps: Vec<u64>,
}

impl BlockMedianTimeContext for FakeMedianTime {
    fn median_block_count(&self) -> u64 {
        11
    }
    fn timestamp(&self, n: u64) -> Option<u64> {
        self.timestamps.get(n as usize).cloned()
    }
    fn ancestor_timestamps(&self, n: u64) -> Vec<u64> {
        self.timestamps[0..=(n as usize)].to_vec()
    }
}

#[test]
pub fn test_since() {
    // use remain flags
    let transaction = TransactionBuilder::default()
        .inputs(vec![CellInput::new(
            OutPoint::new(H256::from_trimmed_hex_str("1").unwrap(), 0),
            0x2000_0000_0000_0000,
            Default::default(),
        )])
        .build();

    let rtx = ResolvedTransaction {
        transaction,
        dep_cells: Vec::new(),
        input_cells: vec![CellStatus::live_output(
            CellOutput::new(capacity_bytes!(50), Vec::new(), Script::default(), None),
            Some(1),
            false,
        )],
    };

    let median_time_context = FakeMedianTime {
        timestamps: vec![0; 11],
    };
    let verifier = ValidSinceVerifier::new(&rtx, &median_time_context, 5);
    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::InvalidValidSince)
    );

    // absolute lock
    let transaction = TransactionBuilder::default()
        .inputs(vec![CellInput::new(
            OutPoint::new(H256::from_trimmed_hex_str("1").unwrap(), 0),
            0x0000_0000_0000_000a,
            Default::default(),
        )])
        .build();

    let rtx = ResolvedTransaction {
        transaction,
        dep_cells: Vec::new(),
        input_cells: vec![CellStatus::live_output(
            CellOutput::new(capacity_bytes!(50), Vec::new(), Script::default(), None),
            Some(1),
            false,
        )],
    };

    let median_time_context = FakeMedianTime {
        timestamps: vec![0; 11],
    };
    let verifier = ValidSinceVerifier::new(&rtx, &median_time_context, 5);
    assert_eq!(verifier.verify().err(), Some(TransactionError::Immature));
    // spent after 10 height
    let verifier = ValidSinceVerifier::new(&rtx, &median_time_context, 10);
    assert!(verifier.verify().is_ok());

    // relative lock
    let transaction = TransactionBuilder::default()
        .inputs(vec![CellInput::new(
            OutPoint::new(H256::from_trimmed_hex_str("1").unwrap(), 0),
            0xc000_0000_0000_0002,
            Default::default(),
        )])
        .build();

    let rtx = ResolvedTransaction {
        transaction,
        dep_cells: Vec::new(),
        input_cells: vec![CellStatus::live_output(
            CellOutput::new(capacity_bytes!(50), Vec::new(), Script::default(), None),
            Some(1),
            false,
        )],
    };

    let verifier = ValidSinceVerifier::new(&rtx, &median_time_context, 4);
    assert_eq!(verifier.verify().err(), Some(TransactionError::Immature));
    // spent after 1024 seconds
    // fake median time: 1124
    let median_time_context = FakeMedianTime {
        timestamps: vec![0, 100_000, 1_124_000, 2_000_000, 3_000_000],
    };
    let verifier = ValidSinceVerifier::new(&rtx, &median_time_context, 4);
    assert!(verifier.verify().is_ok());

    // both
    let transaction = TransactionBuilder::default()
        .inputs(vec![
            CellInput::new(
                OutPoint::new(H256::from_trimmed_hex_str("1").unwrap(), 0),
                0x0000_0000_0000_000a,
                Default::default(),
            ),
            CellInput::new(
                OutPoint::new(H256::from_trimmed_hex_str("1").unwrap(), 0),
                0xc000_0000_0000_0002,
                Default::default(),
            ),
        ])
        .build();

    let rtx = ResolvedTransaction {
        transaction,
        dep_cells: Vec::new(),
        input_cells: vec![CellStatus::live_output(
            CellOutput::new(capacity_bytes!(50), Vec::new(), Script::default(), None),
            Some(1),
            false,
        )],
    };

    let verifier = ValidSinceVerifier::new(&rtx, &median_time_context, 4);
    assert_eq!(verifier.verify().err(), Some(TransactionError::Immature));
    // spent after 1024 seconds and 10 blocks
    // fake median time: 1124
    let median_time_context = FakeMedianTime {
        timestamps: vec![
            0, 1, 2, 3, 4, 100_000, 1_124_000, 2_000_000, 3_000_000, 4_000_000, 5_000_000,
            6_000_000,
        ],
    };
    let verifier = ValidSinceVerifier::new(&rtx, &median_time_context, 10);
    assert!(verifier.verify().is_ok());

    // null should be ok
    let transaction = TransactionBuilder::default()
        .inputs(vec![CellInput::new(
            OutPoint::null(),
            0x0000_0000_0000_000a,
            Default::default(),
        )])
        .build();

    let rtx = ResolvedTransaction {
        transaction,
        dep_cells: Vec::new(),
        input_cells: vec![CellStatus::live_null()],
    };

    let median_time_context = FakeMedianTime {
        timestamps: vec![0; 11],
    };
    let verifier = ValidSinceVerifier::new(&rtx, &median_time_context, 10);
    assert!(verifier.verify().is_ok());
}
