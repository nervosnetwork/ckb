use super::super::transaction_verifier::{
    CapacityVerifier, DuplicateDepsVerifier, EmptyVerifier, MaturityVerifier, OutputsDataVerifier,
    Since, SinceVerifier, SizeVerifier, VersionVerifier,
};
use crate::TransactionError;
use ckb_error::{assert_error_eq, Error};
use ckb_resource::CODE_HASH_DAO;
use ckb_test_chain_utils::MockMedianTime;
use ckb_traits::BlockMedianTimeContext;
use ckb_types::{
    bytes::Bytes,
    constants::TX_VERSION,
    core::{
        capacity_bytes,
        cell::{CellMetaBuilder, ResolvedTransaction},
        BlockNumber, Capacity, ScriptHashType, TransactionBuilder, TransactionInfo,
        TransactionView, Version,
    },
    h256,
    packed::{CellDep, CellInput, CellOutput, OutPoint, Script},
    prelude::*,
    H256,
};
use std::sync::Arc;

#[test]
pub fn test_empty() {
    let transaction = TransactionBuilder::default().build();
    let verifier = EmptyVerifier::new(&transaction);

    assert_error_eq(
        verifier.verify().err(),
        Some(TransactionError::MissingInputsOrOutputs.into()),
    );
}

#[test]
pub fn test_version() {
    let transaction = TransactionBuilder::default()
        .version((TX_VERSION + 1).pack())
        .build();
    let verifier = VersionVerifier::new(&transaction);

    assert_error_eq(
        verifier.verify().err(),
        Some(TransactionError::MismatchedVersion.into()),
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

    assert_error_eq(
        verifier.verify().err(),
        Some(TransactionError::TooLargeSize.into()),
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
        transaction: &transaction,
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
    let verifier = CapacityVerifier::new(&rtx);

    assert_error_eq(
        verifier.verify().err(),
        Some(TransactionError::OccupiedOverflowCapacity.into()),
    );
}

#[test]
pub fn test_skip_dao_capacity_check() {
    let transaction = TransactionBuilder::default()
        .output(
            CellOutput::new_builder()
                .capacity(capacity_bytes!(500).pack())
                .type_(
                    Some(
                        Script::new_builder()
                            .code_hash(CODE_HASH_DAO.pack())
                            .hash_type(ScriptHashType::Data.pack())
                            .build(),
                    )
                    .pack(),
                )
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();

    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![],
        resolved_dep_groups: vec![],
    };
    let verifier = CapacityVerifier::new(&rtx);

    assert!(verifier.verify().is_ok());
}

// inputs immature verify
#[test]
pub fn test_inputs_cellbase_maturity() {
    let transaction = TransactionBuilder::default().build();
    let output = CellOutput::new_builder()
        .capacity(capacity_bytes!(50).pack())
        .build();

    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_cell_deps: Vec::new(),
        resolved_dep_groups: Vec::new(),
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(output.clone(), Bytes::new())
                .transaction_info(MockMedianTime::get_transaction_info(30, 0, 0))
                .build(),
        ],
    };

    let tip_number = 70;
    let cellbase_maturity = 100;
    let verifier1 = MaturityVerifier::new(&rtx, tip_number, cellbase_maturity);

    assert_error_eq(
        verifier1.verify().err(),
        Some(TransactionError::ImmatureCellbase.into()),
    );

    let tip_number = 130;
    let verifier2 = MaturityVerifier::new(&rtx, tip_number, cellbase_maturity);
    assert!(verifier2.verify().is_ok());
}

#[test]
fn test_ignore_genesis_cellbase_maturity() {
    let transaction = TransactionBuilder::default().build();
    let output = CellOutput::new_builder()
        .capacity(capacity_bytes!(50).pack())
        .build();
    // Transaction use genesis cellbase
    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_cell_deps: Vec::new(),
        resolved_dep_groups: Vec::new(),
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(output.clone(), Bytes::new())
                .transaction_info(MockMedianTime::get_transaction_info(0, 0, 0))
                .build(),
        ],
    };
    let tip_number = 70;
    let cellbase_maturity = 100;
    let verifier = MaturityVerifier::new(&rtx, tip_number, cellbase_maturity);
    assert!(verifier.verify().is_ok());
}

// deps immature verify
#[test]
pub fn test_deps_cellbase_maturity() {
    let transaction = TransactionBuilder::default().build();
    let output = CellOutput::new_builder()
        .capacity(capacity_bytes!(50).pack())
        .build();

    // The 1st dep is cellbase, the 2nd one is not.
    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_cell_deps: vec![
            CellMetaBuilder::from_cell_output(output.clone(), Bytes::new())
                .transaction_info(MockMedianTime::get_transaction_info(30, 0, 0))
                .build(),
            CellMetaBuilder::from_cell_output(output.clone(), Bytes::new())
                .transaction_info(MockMedianTime::get_transaction_info(40, 0, 1))
                .build(),
        ],
        resolved_inputs: Vec::new(),
        resolved_dep_groups: vec![],
    };

    let tip_number = 70;
    let cellbase_maturity = 100;
    let verifier = MaturityVerifier::new(&rtx, tip_number, cellbase_maturity);

    assert_error_eq(
        verifier.verify().err(),
        Some(TransactionError::ImmatureCellbase.into()),
    );

    let tip_number = 130;
    let verifier = MaturityVerifier::new(&rtx, tip_number, cellbase_maturity);
    assert!(verifier.verify().is_ok());
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
        transaction: &transaction,
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
    let verifier = CapacityVerifier::new(&rtx);

    assert_error_eq(
        verifier.verify().err(),
        Some(TransactionError::OutputOverflowCapacity.into()),
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

    assert_error_eq(
        verifier.verify().err(),
        Some(TransactionError::DuplicatedDeps.into()),
    );
}

fn verify_since<'a, M>(
    rtx: &'a ResolvedTransaction,
    block_median_time_context: &'a M,
    block_number: BlockNumber,
    epoch_number: BlockNumber,
) -> Result<(), Error>
where
    M: BlockMedianTimeContext,
{
    let parent_hash = Arc::new(MockMedianTime::get_block_hash(block_number - 1));
    SinceVerifier::new(
        rtx,
        block_median_time_context,
        block_number,
        epoch_number,
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
) -> ResolvedTransaction<'_> {
    ResolvedTransaction {
        transaction: &tx,
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
    let rtx =
        create_resolve_tx_with_transaction_info(&tx, MockMedianTime::get_transaction_info(1, 0, 1));

    let median_time_context = MockMedianTime::new(vec![0; 11]);
    assert_error_eq(
        verify_since(&rtx, &median_time_context, 5, 1).err(),
        Some(TransactionError::InvalidSinceFormat.into()),
    );
}

#[test]
pub fn test_absolute_block_number_lock() {
    // absolute lock until block number 0xa
    let tx = create_tx_with_lock(0x0000_0000_0000_000a);
    let rtx =
        create_resolve_tx_with_transaction_info(&tx, MockMedianTime::get_transaction_info(1, 0, 1));
    let median_time_context = MockMedianTime::new(vec![0; 11]);

    assert_error_eq(
        verify_since(&rtx, &median_time_context, 5, 1).err(),
        Some(TransactionError::ImmatureTransaction.into()),
    );
    // spent after 10 height
    assert!(verify_since(&rtx, &median_time_context, 10, 1).is_ok());
}

#[test]
pub fn test_absolute_epoch_number_lock() {
    // absolute lock until epoch number 0xa
    let tx = create_tx_with_lock(0x2000_0000_0000_000a);
    let rtx =
        create_resolve_tx_with_transaction_info(&tx, MockMedianTime::get_transaction_info(1, 0, 1));

    let median_time_context = MockMedianTime::new(vec![0; 11]);
    assert_error_eq(
        verify_since(&rtx, &median_time_context, 5, 1).err(),
        Some(TransactionError::ImmatureTransaction.into()),
    );
    // spent after 10 epoch
    assert!(verify_since(&rtx, &median_time_context, 100, 10).is_ok());
}

#[test]
pub fn test_relative_timestamp_lock() {
    // relative lock timestamp lock
    let tx = create_tx_with_lock(0xc000_0000_0000_0002);
    let rtx =
        create_resolve_tx_with_transaction_info(&tx, MockMedianTime::get_transaction_info(1, 0, 1));

    let median_time_context = MockMedianTime::new(vec![0; 11]);
    assert_error_eq(
        verify_since(&rtx, &median_time_context, 4, 1).err(),
        Some(TransactionError::ImmatureTransaction.into()),
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
    let tx = create_tx_with_lock(0xa000_0000_0000_0001);
    let rtx =
        create_resolve_tx_with_transaction_info(&tx, MockMedianTime::get_transaction_info(1, 1, 1));

    let median_time_context = MockMedianTime::new(vec![0; 11]);

    assert_error_eq(
        verify_since(&rtx, &median_time_context, 4, 1).err(),
        Some(TransactionError::ImmatureTransaction.into()),
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

    let rtx =
        create_resolve_tx_with_transaction_info(&tx, MockMedianTime::get_transaction_info(1, 0, 1));
    // spent after 1024 seconds and 4 blocks (less than 10 blocks)
    // fake median time: 1124
    let median_time_context =
        MockMedianTime::new(vec![0, 100_000, 1_124_000, 2_000_000, 3_000_000]);

    assert_error_eq(
        verify_since(&rtx, &median_time_context, 4, 1).err(),
        Some(TransactionError::ImmatureTransaction.into()),
    );
    // spent after 1024 seconds and 10 blocks
    // fake median time: 1124
    let median_time_context = MockMedianTime::new(vec![
        0, 1, 2, 3, 4, 100_000, 1_124_000, 2_000_000, 3_000_000, 4_000_000, 5_000_000, 6_000_000,
    ]);
    assert!(verify_since(&rtx, &median_time_context, 10, 1).is_ok());
}

#[test]
pub fn test_outputs_data_length_mismatch() {
    let transaction = TransactionBuilder::default()
        .output(Default::default())
        .build();
    let verifier = OutputsDataVerifier::new(&transaction);

    assert_error_eq(
        verifier.verify().err(),
        Some(TransactionError::UnmatchedOutputsDataLength),
    );

    let transaction = TransactionBuilder::default()
        .output(Default::default())
        .output_data(Default::default())
        .build();
    let verifier = OutputsDataVerifier::new(&transaction);

    assert!(verifier.verify().is_ok());
}
