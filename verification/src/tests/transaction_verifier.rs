use super::super::transaction_verifier::{
    CapacityVerifier, DuplicateDepsVerifier, EmptyVerifier, MaturityVerifier, OutputsDataVerifier,
    Since, SinceVerifier, SizeVerifier, VersionVerifier,
};
use crate::error::TransactionErrorSource;
use crate::TransactionError;
use ckb_chain_spec::{build_genesis_type_id_script, OUTPUT_INDEX_DAO};
use ckb_error::{assert_error_eq, Error};
use ckb_test_chain_utils::MockMedianTime;
use ckb_traits::BlockMedianTimeContext;
use ckb_types::{
    bytes::Bytes,
    constants::TX_VERSION,
    core::{
        capacity_bytes,
        cell::{CellMetaBuilder, ResolvedTransaction},
        BlockNumber, Capacity, EpochNumber, EpochNumberWithFraction, TransactionBuilder,
        TransactionInfo, TransactionView, Version,
    },
    h256,
    packed::{CellDep, CellInput, CellOutput, OutPoint},
    prelude::*,
    H256,
};
use std::sync::Arc;

#[test]
pub fn test_empty() {
    let transaction = TransactionBuilder::default().build();
    let verifier = EmptyVerifier::new(&transaction);

    assert_error_eq!(verifier.verify().unwrap_err(), TransactionError::Empty);
}

#[test]
pub fn test_version() {
    let transaction = TransactionBuilder::default()
        .version((TX_VERSION + 1).pack())
        .build();
    let verifier = VersionVerifier::new(&transaction, TX_VERSION);

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        TransactionError::MismatchedVersion,
    );
}

#[test]
pub fn test_exceeded_maximum_block_bytes() {
    let data: Bytes = vec![1; 500].into();
    let transaction = TransactionBuilder::default()
        .version((Version::default() + 1).pack())
        .output(
            CellOutput::new_builder()
                .capacity(capacity_bytes!(50).pack())
                .build(),
        )
        .output_data(data.pack())
        .build();
    let verifier = SizeVerifier::new(&transaction, 100);

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        TransactionError::ExceededMaximumBlockBytes,
    );
}

#[test]
pub fn test_capacity_outofbound() {
    let data = Bytes::from(vec![1; 51]);
    let transaction = TransactionBuilder::default()
        .output(
            CellOutput::new_builder()
                .capacity(capacity_bytes!(50).pack())
                .build(),
        )
        .output_data(data.pack())
        .build();

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![CellMetaBuilder::from_cell_output(
            CellOutput::new_builder()
                .capacity(capacity_bytes!(50).pack())
                .build(),
            Bytes::new(),
        )
        .build()],
        resolved_dep_groups: vec![],
    };
    let dao_type_hash = build_genesis_type_id_script(OUTPUT_INDEX_DAO).calc_script_hash();
    let verifier = CapacityVerifier::new(&rtx, Some(dao_type_hash));

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        TransactionError::InsufficientCellCapacity {
            source: TransactionErrorSource::Outputs,
            index: 0,
            capacity: capacity_bytes!(50),
            occupied_capacity: capacity_bytes!(92),
        }
    );
}

#[test]
pub fn test_skip_dao_capacity_check() {
    let dao_type_script = build_genesis_type_id_script(OUTPUT_INDEX_DAO);
    let transaction = TransactionBuilder::default()
        .output(
            CellOutput::new_builder()
                .capacity(capacity_bytes!(500).pack())
                .type_(Some(dao_type_script.clone()).pack())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![],
        resolved_dep_groups: vec![],
    };
    let verifier = CapacityVerifier::new(&rtx, Some(dao_type_script.calc_script_hash()));

    assert!(verifier.verify().is_ok());
}

// inputs immature verify
#[test]
pub fn test_inputs_cellbase_maturity() {
    let transaction = TransactionBuilder::default().build();
    let output = CellOutput::new_builder()
        .capacity(capacity_bytes!(50).pack())
        .build();
    let base_epoch = EpochNumberWithFraction::new(10, 0, 10);
    let cellbase_maturity = EpochNumberWithFraction::new(5, 0, 1);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_dep_groups: Vec::new(),
        resolved_inputs: vec![CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(MockMedianTime::get_transaction_info(30, base_epoch, 0))
            .build()],
    };

    let mut current_epoch = EpochNumberWithFraction::new(0, 0, 10);
    let threshold = cellbase_maturity.to_rational() + base_epoch.to_rational();
    while current_epoch.number() < cellbase_maturity.number() + base_epoch.number() + 5 {
        let verifier = MaturityVerifier::new(&rtx, current_epoch, cellbase_maturity);
        let current = current_epoch.to_rational();
        if current < threshold {
            assert_error_eq!(
                verifier.verify().unwrap_err(),
                TransactionError::CellbaseImmaturity {
                    source: TransactionErrorSource::Inputs,
                    index: 0
                },
                "base_epoch = {}, current_epoch = {}, cellbase_maturity = {}",
                base_epoch,
                current_epoch,
                cellbase_maturity
            );
        } else {
            assert!(
                verifier.verify().is_ok(),
                "base_epoch = {}, current_epoch = {}, cellbase_maturity = {}",
                base_epoch,
                current_epoch,
                cellbase_maturity
            );
        }
        {
            let number = current_epoch.number();
            let length = current_epoch.length();
            let index = current_epoch.index();
            current_epoch = if index == length {
                EpochNumberWithFraction::new(number + 1, 0, length)
            } else {
                EpochNumberWithFraction::new(number, index + 1, length)
            };
        }
    }
}

#[test]
fn test_ignore_genesis_cellbase_maturity() {
    let transaction = TransactionBuilder::default().build();
    let output = CellOutput::new_builder()
        .capacity(capacity_bytes!(50).pack())
        .build();
    let base_epoch = EpochNumberWithFraction::new(0, 0, 10);
    let cellbase_maturity = EpochNumberWithFraction::new(5, 0, 1);
    // Transaction use genesis cellbase
    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_dep_groups: Vec::new(),
        resolved_inputs: vec![CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(MockMedianTime::get_transaction_info(0, base_epoch, 0))
            .build()],
    };

    let mut current_epoch = EpochNumberWithFraction::new(0, 0, 10);
    while current_epoch.number() < cellbase_maturity.number() + base_epoch.number() + 5 {
        let verifier = MaturityVerifier::new(&rtx, current_epoch, cellbase_maturity);
        assert!(
            verifier.verify().is_ok(),
            "base_epoch = {}, current_epoch = {}, cellbase_maturity = {}",
            base_epoch,
            current_epoch,
            cellbase_maturity
        );
        {
            let number = current_epoch.number();
            let length = current_epoch.length();
            let index = current_epoch.index();
            current_epoch = if index == length {
                EpochNumberWithFraction::new(number + 1, 0, length)
            } else {
                EpochNumberWithFraction::new(number, index + 1, length)
            };
        }
    }
}

// deps immature verify
#[test]
pub fn test_deps_cellbase_maturity() {
    let transaction = TransactionBuilder::default().build();
    let output = CellOutput::new_builder()
        .capacity(capacity_bytes!(50).pack())
        .build();

    let base_epoch = EpochNumberWithFraction::new(0, 0, 10);
    let cellbase_maturity = EpochNumberWithFraction::new(5, 0, 1);

    // The 1st dep is cellbase, the 2nd one is not.
    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![
            CellMetaBuilder::from_cell_output(output.clone(), Bytes::new())
                .transaction_info(MockMedianTime::get_transaction_info(30, base_epoch, 0))
                .build(),
            CellMetaBuilder::from_cell_output(output, Bytes::new())
                .transaction_info(MockMedianTime::get_transaction_info(40, base_epoch, 1))
                .build(),
        ],
        resolved_inputs: Vec::new(),
        resolved_dep_groups: vec![],
    };

    let mut current_epoch = EpochNumberWithFraction::new(0, 0, 10);
    let threshold = cellbase_maturity.to_rational() + base_epoch.to_rational();
    while current_epoch.number() < cellbase_maturity.number() + base_epoch.number() + 5 {
        let verifier = MaturityVerifier::new(&rtx, current_epoch, cellbase_maturity);
        let current = current_epoch.to_rational();
        if current < threshold {
            assert_error_eq!(
                verifier.verify().unwrap_err(),
                TransactionError::CellbaseImmaturity {
                    source: TransactionErrorSource::CellDeps,
                    index: 0
                },
                "base_epoch = {}, current_epoch = {}, cellbase_maturity = {}",
                base_epoch,
                current_epoch,
                cellbase_maturity,
            );
        } else {
            assert!(
                verifier.verify().is_ok(),
                "base_epoch = {}, current_epoch = {}, cellbase_maturity = {}",
                base_epoch,
                current_epoch,
                cellbase_maturity
            );
        }
        {
            let number = current_epoch.number();
            let length = current_epoch.length();
            let index = current_epoch.index();
            current_epoch = if index == length {
                EpochNumberWithFraction::new(number + 1, 0, length)
            } else {
                EpochNumberWithFraction::new(number, index + 1, length)
            };
        }
    }
}

#[test]
pub fn test_capacity_invalid() {
    // The outputs capacity is 50 + 100 = 150
    let transaction = TransactionBuilder::default()
        .outputs(vec![
            CellOutput::new_builder()
                .capacity(capacity_bytes!(50).pack())
                .build(),
            CellOutput::new_builder()
                .capacity(capacity_bytes!(100).pack())
                .build(),
        ])
        .outputs_data(vec![Bytes::new().pack(); 2])
        .build();

    // The inputs capacity is 49 + 100 = 149,
    // is less than outputs capacity
    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(49).pack())
                    .build(),
                Bytes::new(),
            )
            .build(),
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(100).pack())
                    .build(),
                Bytes::new(),
            )
            .build(),
        ],
        resolved_dep_groups: vec![],
    };
    let dao_type_hash = build_genesis_type_id_script(OUTPUT_INDEX_DAO).calc_script_hash();
    let verifier = CapacityVerifier::new(&rtx, Some(dao_type_hash));

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        TransactionError::OutputsSumOverflow,
    );
}

#[test]
pub fn test_duplicate_deps() {
    let out_point = OutPoint::new(h256!("0x1").pack(), 0);
    let cell_dep = CellDep::new_builder().out_point(out_point).build();
    let transaction = TransactionBuilder::default()
        .cell_deps(vec![cell_dep.clone(), cell_dep])
        .build();

    let verifier = DuplicateDepsVerifier::new(&transaction);

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        TransactionError::DuplicateDeps,
    );
}

fn verify_since<'a, M>(
    rtx: &'a ResolvedTransaction,
    block_median_time_context: &'a M,
    block_number: BlockNumber,
    epoch_number: EpochNumber,
) -> Result<(), Error>
where
    M: BlockMedianTimeContext,
{
    let parent_hash = Arc::new(MockMedianTime::get_block_hash(block_number - 1));
    SinceVerifier::new(
        rtx,
        block_median_time_context,
        block_number,
        EpochNumberWithFraction::new(epoch_number, 0, 10),
        parent_hash.as_ref().to_owned(),
    )
    .verify()
}

#[test]
fn test_since() {
    let valids = vec![
        0x0000_0000_0000_0001,
        0x2000_0000_0000_0001,
        0x4000_0000_0000_0001,
        0x8000_0000_0000_0001,
        0xa000_0000_0000_0001,
        0xc000_0000_0000_0001,
    ];

    for v in valids.into_iter() {
        let since = Since(v);
        assert_eq!(since.flags_is_valid(), true);
    }

    let invalids = vec![
        0x0100_0000_0000_0001,
        0x1000_0000_0000_0001,
        0xd000_0000_0000_0001,
    ];

    for v in invalids.into_iter() {
        let since = Since(v);
        assert_eq!(since.flags_is_valid(), false);
    }
}

fn create_tx_with_lock(since: u64) -> TransactionView {
    TransactionBuilder::default()
        .inputs(vec![CellInput::new(
            OutPoint::new(h256!("0x1").pack(), 0),
            since,
        )])
        .build()
}

fn create_resolve_tx_with_transaction_info(
    tx: &TransactionView,
    transaction_info: TransactionInfo,
) -> ResolvedTransaction {
    ResolvedTransaction {
        transaction: tx.clone(),
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![CellMetaBuilder::from_cell_output(
            CellOutput::new_builder()
                .capacity(capacity_bytes!(50).pack())
                .build(),
            Bytes::new(),
        )
        .transaction_info(transaction_info)
        .build()],
        resolved_dep_groups: vec![],
    }
}

#[test]
fn test_invalid_since_verify() {
    // use remain flags
    let tx = create_tx_with_lock(0x0100_0000_0000_0001);
    let rtx = create_resolve_tx_with_transaction_info(
        &tx,
        MockMedianTime::get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
    );

    let median_time_context = MockMedianTime::new(vec![0; 11]);
    assert_error_eq!(
        verify_since(&rtx, &median_time_context, 5, 1).unwrap_err(),
        TransactionError::InvalidSince,
    );
}

#[test]
fn test_valid_zero_length_since() {
    // use remain flags
    let tx = create_tx_with_lock(0xa000_0000_0000_0000);
    let rtx = create_resolve_tx_with_transaction_info(
        &tx,
        MockMedianTime::get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
    );

    let median_time_context = MockMedianTime::new(vec![0; 11]);
    assert!(verify_since(&rtx, &median_time_context, 5, 1).is_ok(),);
}

#[test]
fn test_fraction_epoch_since_verify() {
    let tx = create_tx_with_lock(0x2000_0a00_0500_0010);
    let rtx = create_resolve_tx_with_transaction_info(
        &tx,
        MockMedianTime::get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
    );
    let median_time_context = MockMedianTime::new(vec![0; 11]);
    let block_number = 1000;
    let parent_hash = Arc::new(MockMedianTime::get_block_hash(block_number - 1));

    let result = SinceVerifier::new(
        &rtx,
        &median_time_context,
        block_number,
        EpochNumberWithFraction::new(16, 1, 10),
        parent_hash.as_ref().to_owned(),
    )
    .verify();
    assert_error_eq!(result.unwrap_err(), TransactionError::Immature);

    let result = SinceVerifier::new(
        &rtx,
        &median_time_context,
        block_number,
        EpochNumberWithFraction::new(16, 5, 10),
        parent_hash.as_ref().to_owned(),
    )
    .verify();
    assert!(result.is_ok());
}

#[test]
pub fn test_absolute_block_number_lock() {
    // absolute lock until block number 0xa
    let tx = create_tx_with_lock(0x0000_0000_0000_000a);
    let rtx = create_resolve_tx_with_transaction_info(
        &tx,
        MockMedianTime::get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
    );
    let median_time_context = MockMedianTime::new(vec![0; 11]);

    assert_error_eq!(
        verify_since(&rtx, &median_time_context, 5, 1).unwrap_err(),
        TransactionError::Immature,
    );
    // spent after 10 height
    assert!(verify_since(&rtx, &median_time_context, 10, 1).is_ok());
}

#[test]
pub fn test_absolute_epoch_number_lock() {
    // absolute lock until epoch number 0xa
    let tx = create_tx_with_lock(0x2000_0100_0000_000a);
    let rtx = create_resolve_tx_with_transaction_info(
        &tx,
        MockMedianTime::get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
    );

    let median_time_context = MockMedianTime::new(vec![0; 11]);
    assert_error_eq!(
        verify_since(&rtx, &median_time_context, 5, 1).unwrap_err(),
        TransactionError::Immature,
    );
    // spent after 10 epoch
    assert!(verify_since(&rtx, &median_time_context, 100, 10).is_ok());
}

#[test]
pub fn test_relative_timestamp_lock() {
    // relative lock timestamp lock
    let tx = create_tx_with_lock(0xc000_0000_0000_0002);
    let rtx = create_resolve_tx_with_transaction_info(
        &tx,
        MockMedianTime::get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
    );

    let median_time_context = MockMedianTime::new(vec![0; 11]);
    assert_error_eq!(
        verify_since(&rtx, &median_time_context, 4, 1).unwrap_err(),
        TransactionError::Immature,
    );

    // spent after 1024 seconds
    // fake median time: 1124
    let median_time_context =
        MockMedianTime::new(vec![0, 100_000, 1_124_000, 2_000_000, 3_000_000]);
    assert!(verify_since(&rtx, &median_time_context, 4, 1).is_ok());
}

#[test]
pub fn test_relative_epoch() {
    // next epoch
    let tx = create_tx_with_lock(0xa000_1000_0000_0002);
    let rtx = create_resolve_tx_with_transaction_info(
        &tx,
        MockMedianTime::get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
    );

    let median_time_context = MockMedianTime::new(vec![0; 11]);

    assert_error_eq!(
        verify_since(&rtx, &median_time_context, 4, 1).unwrap_err(),
        TransactionError::Immature,
    );

    assert!(verify_since(&rtx, &median_time_context, 4, 2).is_ok());
}

#[test]
pub fn test_since_both() {
    // both
    let tx = TransactionBuilder::default()
        .inputs(vec![
            // absolute lock until epoch number 0xa
            CellInput::new(OutPoint::new(h256!("0x1").pack(), 0), 0x0000_0000_0000_000a),
            // relative lock until after 2 blocks
            CellInput::new(OutPoint::new(h256!("0x1").pack(), 0), 0xc000_0000_0000_0002),
        ])
        .build();

    let rtx = create_resolve_tx_with_transaction_info(
        &tx,
        MockMedianTime::get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
    );
    // spent after 1024 seconds and 4 blocks (less than 10 blocks)
    // fake median time: 1124
    let median_time_context =
        MockMedianTime::new(vec![0, 100_000, 1_124_000, 2_000_000, 3_000_000]);

    assert_error_eq!(
        verify_since(&rtx, &median_time_context, 4, 1).unwrap_err(),
        TransactionError::Immature,
    );
    // spent after 1024 seconds and 10 blocks
    // fake median time: 1124
    let median_time_context = MockMedianTime::new(vec![
        0, 1, 2, 3, 4, 100_000, 1_124_000, 2_000_000, 3_000_000, 4_000_000, 5_000_000, 6_000_000,
    ]);
    assert!(verify_since(&rtx, &median_time_context, 10, 1).is_ok());
}

#[test]
fn test_since_overflow() {
    // use max value for each flag
    for flag in &[
        0b0000_0000u64, // absolute & block
        0b1000_0000u64, // relative & block
        0b0010_0000u64, // absolute & epoch
        0b1010_0000u64, // relative & epoch
        0b0100_0000u64, // absolute & time
        0b1100_0000u64, // relative & time
    ] {
        let tx = create_tx_with_lock((flag << 56) + 0xffff_ffff_ffffu64);
        let rtx = create_resolve_tx_with_transaction_info(
            &tx,
            MockMedianTime::get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
        );

        let median_time_context = MockMedianTime::new(vec![0; 11]);
        assert_error_eq!(
            verify_since(&rtx, &median_time_context, 5, 1).unwrap_err(),
            TransactionError::Immature,
        );
    }
}

#[test]
pub fn test_outputs_data_length_mismatch() {
    let transaction = TransactionBuilder::default()
        .output(Default::default())
        .build();
    let verifier = OutputsDataVerifier::new(&transaction);

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        TransactionError::OutputsDataLengthMismatch,
    );

    let transaction = TransactionBuilder::default()
        .output(Default::default())
        .output_data(Default::default())
        .build();
    let verifier = OutputsDataVerifier::new(&transaction);

    assert!(verifier.verify().is_ok());
}
