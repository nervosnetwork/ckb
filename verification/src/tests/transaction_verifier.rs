use super::super::transaction_verifier::{
    CapacityVerifier, DuplicateDepsVerifier, EmptyVerifier, MaturityVerifier, OutputsDataVerifier,
    Since, SinceVerifier, SizeVerifier, VersionVerifier,
};
use crate::error::TransactionError;
use ckb_core::cell::{CellMetaBuilder, ResolvedTransaction};
use ckb_core::extras::TransactionInfo;
use ckb_core::script::{Script, ScriptHashType};
use ckb_core::transaction::{
    CellDep, CellInput, CellOutputBuilder, OutPoint, Transaction, TransactionBuilder, TX_VERSION,
};
use ckb_core::{capacity_bytes, BlockNumber, Bytes, Capacity, Version};
use ckb_resource::CODE_HASH_DAO;
use ckb_test_chain_utils::MockMedianTime;
use ckb_traits::BlockMedianTimeContext;
use numext_fixed_hash::{h256, H256};
use std::sync::Arc;

#[test]
pub fn test_empty() {
    let transaction = TransactionBuilder::default().build();
    let verifier = EmptyVerifier::new(&transaction);

    assert_eq!(verifier.verify().err(), Some(TransactionError::Empty));
}

#[test]
pub fn test_version() {
    let transaction = TransactionBuilder::default()
        .version(TX_VERSION + 1)
        .build();
    let verifier = VersionVerifier::new(&transaction);

    assert_eq!(verifier.verify().err(), Some(TransactionError::Version));
}

#[test]
pub fn test_exceeded_maximum_block_bytes() {
    let data: Bytes = vec![1; 500].into();
    let transaction = TransactionBuilder::default()
        .version(Version::default() + 1)
        .output(
            CellOutputBuilder::from_data(&data)
                .capacity(capacity_bytes!(50))
                .build(),
        )
        .output_data(data)
        .build();
    let verifier = SizeVerifier::new(&transaction, 100);

    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::ExceededMaximumBlockBytes)
    );
}

#[test]
pub fn test_capacity_outofbound() {
    let data = Bytes::from(vec![1; 51]);
    let transaction = TransactionBuilder::default()
        .output(
            CellOutputBuilder::from_data(&data)
                .capacity(capacity_bytes!(50))
                .build(),
        )
        .output_data(data)
        .build();

    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![CellMetaBuilder::from_cell_output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(50))
                .build(),
            Bytes::new(),
        )
        .build()],
    };
    let verifier = CapacityVerifier::new(&rtx);

    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::InsufficientCellCapacity)
    );
}

#[test]
pub fn test_skip_dao_capacity_check() {
    let data = Bytes::from(vec![1; 10]);
    let transaction = TransactionBuilder::default()
        .output(
            CellOutputBuilder::from_data(&data)
                .capacity(capacity_bytes!(500))
                .type_(Some(Script::new(
                    vec![],
                    CODE_HASH_DAO,
                    ScriptHashType::Data,
                )))
                .build(),
        )
        .output_data(Bytes::new())
        .build();

    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![],
    };
    let verifier = CapacityVerifier::new(&rtx);

    assert!(verifier.verify().is_ok());
}

// inputs immature verify
#[test]
pub fn test_inputs_cellbase_maturity() {
    let transaction = TransactionBuilder::default().build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(50))
        .build();

    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(output.clone(), Bytes::new())
                .transaction_info(MockMedianTime::get_transaction_info(30, 0, 0))
                .build(),
        ],
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

// deps immature verify
#[test]
pub fn test_deps_cellbase_maturity() {
    let transaction = TransactionBuilder::default().build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(50))
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
    // The outputs capacity is 50 + 100 = 150
    let transaction = TransactionBuilder::default()
        .outputs(vec![
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(50))
                .build(),
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(100))
                .build(),
        ])
        .outputs_data(vec![Bytes::new(); 2])
        .build();

    // The inputs capacity is 49 + 100 = 149,
    // is less than outputs capacity
    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(49))
                    .build(),
                Bytes::new(),
            )
            .build(),
            CellMetaBuilder::from_cell_output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(100))
                    .build(),
                Bytes::new(),
            )
            .build(),
        ],
    };
    let verifier = CapacityVerifier::new(&rtx);

    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::OutputsSumOverflow)
    );
}

#[test]
pub fn test_duplicate_deps() {
    let out_point = OutPoint::new(h256!("0x1"), 0);
    let cell_dep = CellDep::new_cell(out_point);
    let transaction = TransactionBuilder::default()
        .cell_deps(vec![cell_dep.clone(), cell_dep])
        .build();

    let verifier = DuplicateDepsVerifier::new(&transaction);

    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::DuplicateDeps)
    );
}

fn verify_since<'a, M>(
    rtx: &'a ResolvedTransaction,
    block_median_time_context: &'a M,
    block_number: BlockNumber,
    epoch_number: BlockNumber,
) -> Result<(), TransactionError>
where
    M: BlockMedianTimeContext,
{
    let parent_hash = Arc::new(MockMedianTime::get_block_hash(block_number - 1));
    SinceVerifier::new(
        rtx,
        block_median_time_context,
        block_number,
        epoch_number,
        &parent_hash,
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

fn create_tx_with_lock(since: u64) -> Transaction {
    TransactionBuilder::default()
        .inputs(vec![CellInput::new(OutPoint::new(h256!("0x1"), 0), since)])
        .build()
}

fn create_resolve_tx_with_transaction_info(
    tx: &Transaction,
    transaction_info: TransactionInfo,
) -> ResolvedTransaction<'_> {
    ResolvedTransaction {
        transaction: &tx,
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![CellMetaBuilder::from_cell_output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(50))
                .build(),
            Bytes::new(),
        )
        .transaction_info(transaction_info)
        .build()],
    }
}

#[test]
fn test_invalid_since_verify() {
    // use remain flags
    let tx = create_tx_with_lock(0x0100_0000_0000_0001);
    let rtx =
        create_resolve_tx_with_transaction_info(&tx, MockMedianTime::get_transaction_info(1, 0, 1));

    let median_time_context = MockMedianTime::new(vec![0; 11]);
    assert_eq!(
        verify_since(&rtx, &median_time_context, 5, 1).err(),
        Some(TransactionError::InvalidSince)
    );
}

#[test]
pub fn test_absolute_block_number_lock() {
    // absolute lock until block number 0xa
    let tx = create_tx_with_lock(0x0000_0000_0000_000a);
    let rtx =
        create_resolve_tx_with_transaction_info(&tx, MockMedianTime::get_transaction_info(1, 0, 1));
    let median_time_context = MockMedianTime::new(vec![0; 11]);

    assert_eq!(
        verify_since(&rtx, &median_time_context, 5, 1).err(),
        Some(TransactionError::Immature)
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
    assert_eq!(
        verify_since(&rtx, &median_time_context, 5, 1).err(),
        Some(TransactionError::Immature)
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
    assert_eq!(
        verify_since(&rtx, &median_time_context, 4, 1).err(),
        Some(TransactionError::Immature)
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

    assert_eq!(
        verify_since(&rtx, &median_time_context, 4, 1).err(),
        Some(TransactionError::Immature)
    );

    assert!(verify_since(&rtx, &median_time_context, 4, 2).is_ok());
}

#[test]
pub fn test_since_both() {
    // both
    let transaction = TransactionBuilder::default()
        .inputs(vec![
            // absolute lock until epoch number 0xa
            CellInput::new(OutPoint::new(h256!("0x1"), 0), 0x0000_0000_0000_000a),
            // relative lock until after 2 blocks
            CellInput::new(OutPoint::new(h256!("0x1"), 0), 0xc000_0000_0000_0002),
        ])
        .build();

    let rtx = create_resolve_tx_with_transaction_info(
        &transaction,
        MockMedianTime::get_transaction_info(1, 0, 1),
    );
    // spent after 1024 seconds and 4 blocks (less than 10 blocks)
    // fake median time: 1124
    let median_time_context =
        MockMedianTime::new(vec![0, 100_000, 1_124_000, 2_000_000, 3_000_000]);

    assert_eq!(
        verify_since(&rtx, &median_time_context, 4, 1).err(),
        Some(TransactionError::Immature)
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

    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::OutputsDataLengthMismatch)
    );

    let transaction = TransactionBuilder::default()
        .output(Default::default())
        .output_data(Default::default())
        .build();
    let verifier = OutputsDataVerifier::new(&transaction);

    assert!(verifier.verify().is_ok());
}

#[test]
pub fn test_outputs_data_hash_mismatch() {
    let data: Bytes = Bytes::from(&b"Hello Wrold"[..]);
    let transaction = TransactionBuilder::default()
        .output(Default::default())
        .output_data(data.clone())
        .build();
    let verifier = OutputsDataVerifier::new(&transaction);

    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::OutputDataHashMismatch)
    );

    let transaction = TransactionBuilder::default()
        .output(CellOutputBuilder::from_data(&data).build())
        .output_data(data)
        .build();
    let verifier = OutputsDataVerifier::new(&transaction);

    assert!(verifier.verify().is_ok());
}
