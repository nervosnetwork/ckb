use super::super::transaction_verifier::{
    CapacityVerifier, DuplicateDepsVerifier, EmptyVerifier, MaturityVerifier, SinceVerifier,
};
use crate::error::TransactionError;
use ckb_core::cell::{BlockInfo, CellMeta, CellMetaBuilder, ResolvedOutPoint, ResolvedTransaction};
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, TransactionBuilder};
use ckb_core::{capacity_bytes, BlockNumber, Bytes, Capacity};
use ckb_traits::BlockMedianTimeContext;
use numext_fixed_hash::{h256, H256};

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
            Bytes::from(vec![1; 51]),
            Script::default(),
            None,
        ))
        .build();

    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_deps: Vec::new(),
        resolved_inputs: vec![ResolvedOutPoint::cell_only(CellMeta::from(
            &CellOutput::new(capacity_bytes!(50), Bytes::new(), Script::default(), None),
        ))],
    };
    let verifier = CapacityVerifier::new(&rtx);

    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::InsufficientCellCapacity)
    );
}

#[test]
pub fn test_skip_dao_capacity_check() {
    let transaction = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new_issuing_dao(), 0, vec![]))
        .output(CellOutput::new(
            capacity_bytes!(500),
            Bytes::from(vec![1; 10]),
            Script::default(),
            None,
        ))
        .build();

    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_deps: Vec::new(),
        resolved_inputs: vec![ResolvedOutPoint::issuing_dao()],
    };
    let verifier = CapacityVerifier::new(&rtx);

    assert!(verifier.verify().is_ok());
}

#[test]
pub fn test_cellbase_maturity() {
    let transaction = TransactionBuilder::default()
        .output(CellOutput::new(
            capacity_bytes!(50),
            vec![1; 51].into(),
            Script::default(),
            None,
        ))
        .build();

    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_deps: Vec::new(),
        resolved_inputs: vec![ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(CellOutput::new(
                capacity_bytes!(50),
                Bytes::new(),
                Script::default(),
                None,
            ))
            .block_info(BlockInfo::new(30, 0))
            .cellbase(true)
            .build(),
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
            CellOutput::new(
                capacity_bytes!(50),
                Bytes::default(),
                Script::default(),
                None,
            ),
            CellOutput::new(
                capacity_bytes!(100),
                Bytes::default(),
                Script::default(),
                None,
            ),
        ])
        .build();

    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_deps: Vec::new(),
        resolved_inputs: vec![
            ResolvedOutPoint::cell_only(CellMeta::from(&CellOutput::new(
                capacity_bytes!(49),
                Bytes::default(),
                Script::default(),
                None,
            ))),
            ResolvedOutPoint::cell_only(CellMeta::from(&CellOutput::new(
                capacity_bytes!(100),
                Bytes::default(),
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
pub fn test_duplicate_deps() {
    let transaction = TransactionBuilder::default()
        .deps(vec![
            OutPoint::new_cell(h256!("0x1"), 0),
            OutPoint::new_cell(h256!("0x1"), 0),
        ])
        .build();

    let verifier = DuplicateDepsVerifier::new(&transaction);

    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::DuplicateDeps)
    );
}

struct FakeMedianTime {
    timestamps: Vec<u64>,
}

impl BlockMedianTimeContext for FakeMedianTime {
    fn median_block_count(&self) -> u64 {
        11
    }

    fn timestamp_and_parent(&self, block_hash: &H256) -> (u64, H256) {
        for i in 0..self.timestamps.len() {
            if &self.get_block_hash(i as u64).unwrap() == block_hash {
                if i == 0 {
                    return (self.timestamps[i], H256::zero());
                } else {
                    return (
                        self.timestamps[i],
                        self.get_block_hash(i as u64 - 1).unwrap(),
                    );
                }
            }
        }
        unreachable!()
    }

    fn get_block_hash(&self, block_number: BlockNumber) -> Option<H256> {
        let vec: Vec<u8> = (0..32).map(|_| block_number as u8).collect();
        Some(H256::from_slice(vec.as_slice()).unwrap())
    }
}

#[test]
pub fn test_since() {
    // use remain flags
    let transaction = TransactionBuilder::default()
        .inputs(vec![CellInput::new(
            OutPoint::new_cell(h256!("0x1"), 0),
            0x1000_0000_0000_0000,
            Default::default(),
        )])
        .build();

    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_deps: Vec::new(),
        resolved_inputs: vec![ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(CellOutput::new(
                capacity_bytes!(50),
                Bytes::new(),
                Script::default(),
                None,
            ))
            .block_info(BlockInfo::new(1, 0))
            .build(),
        )],
    };

    let median_time_context = FakeMedianTime {
        timestamps: vec![0; 11],
    };
    let verifier = SinceVerifier::new(&rtx, &median_time_context, 5, 1);
    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::InvalidSince)
    );

    // absolute lock
    let transaction = TransactionBuilder::default()
        .inputs(vec![CellInput::new(
            OutPoint::new_cell(h256!("0x1"), 0),
            0x0000_0000_0000_000a,
            Default::default(),
        )])
        .build();

    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_deps: Vec::new(),
        resolved_inputs: vec![ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(CellOutput::new(
                capacity_bytes!(50),
                Bytes::new(),
                Script::default(),
                None,
            ))
            .block_info(BlockInfo::new(1, 0))
            .build(),
        )],
    };

    let median_time_context = FakeMedianTime {
        timestamps: vec![0; 11],
    };
    let verifier = SinceVerifier::new(&rtx, &median_time_context, 5, 1);
    assert_eq!(verifier.verify().err(), Some(TransactionError::Immature));
    // spent after 10 height
    let verifier = SinceVerifier::new(&rtx, &median_time_context, 10, 1);
    assert!(verifier.verify().is_ok());

    // relative lock
    let transaction = TransactionBuilder::default()
        .inputs(vec![CellInput::new(
            OutPoint::new_cell(h256!("0x1"), 0),
            0xc000_0000_0000_0002,
            Default::default(),
        )])
        .build();

    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_deps: Vec::new(),
        resolved_inputs: vec![ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(CellOutput::new(
                capacity_bytes!(50),
                Bytes::new(),
                Script::default(),
                None,
            ))
            .block_info(BlockInfo::new(1, 0))
            .build(),
        )],
    };

    let verifier = SinceVerifier::new(&rtx, &median_time_context, 4, 1);
    assert_eq!(verifier.verify().err(), Some(TransactionError::Immature));
    // spent after 1024 seconds
    // fake median time: 1124
    let median_time_context = FakeMedianTime {
        timestamps: vec![0, 100_000, 1_124_000, 2_000_000, 3_000_000],
    };
    let verifier = SinceVerifier::new(&rtx, &median_time_context, 4, 1);
    assert!(verifier.verify().is_ok());

    // both
    let transaction = TransactionBuilder::default()
        .inputs(vec![
            CellInput::new(
                OutPoint::new_cell(h256!("0x1"), 0),
                0x0000_0000_0000_000a,
                Default::default(),
            ),
            CellInput::new(
                OutPoint::new_cell(h256!("0x1"), 0),
                0xc000_0000_0000_0002,
                Default::default(),
            ),
        ])
        .build();

    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_deps: Vec::new(),
        resolved_inputs: vec![ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(CellOutput::new(
                capacity_bytes!(50),
                Bytes::default(),
                Script::default(),
                None,
            ))
            .block_info(BlockInfo::new(1, 0))
            .build(),
        )],
    };

    let verifier = SinceVerifier::new(&rtx, &median_time_context, 4, 1);
    assert_eq!(verifier.verify().err(), Some(TransactionError::Immature));
    // spent after 1024 seconds and 10 blocks
    // fake median time: 1124
    let median_time_context = FakeMedianTime {
        timestamps: vec![
            0, 1, 2, 3, 4, 100_000, 1_124_000, 2_000_000, 3_000_000, 4_000_000, 5_000_000,
            6_000_000,
        ],
    };
    let verifier = SinceVerifier::new(&rtx, &median_time_context, 10, 1);
    assert!(verifier.verify().is_ok());
    // next epoch
    let transaction = TransactionBuilder::default()
        .inputs(vec![CellInput::new(
            OutPoint::new_cell(h256!("0x1"), 0),
            0xa000_0000_0000_0001,
            Default::default(),
        )])
        .build();

    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_deps: Vec::new(),
        resolved_inputs: vec![ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(CellOutput::new(
                capacity_bytes!(50),
                Bytes::new(),
                Script::default(),
                None,
            ))
            .block_info(BlockInfo::new(1, 1))
            .build(),
        )],
    };

    let verifier = SinceVerifier::new(&rtx, &median_time_context, 4, 1);
    assert_eq!(verifier.verify().err(), Some(TransactionError::Immature));
    let verifier = SinceVerifier::new(&rtx, &median_time_context, 4, 2);
    assert!(verifier.verify().is_ok());

    // invalid since flags
    let transaction = TransactionBuilder::default()
        .inputs(vec![CellInput::new(
            OutPoint::new_cell(h256!("0x1"), 0),
            0x6000_0000_0000_0001,
            Default::default(),
        )])
        .build();

    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_deps: Vec::new(),
        resolved_inputs: vec![ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(CellOutput::new(
                capacity_bytes!(50),
                Bytes::new(),
                Script::default(),
                None,
            ))
            .block_info(BlockInfo::new(1, 1))
            .build(),
        )],
    };

    let verifier = SinceVerifier::new(&rtx, &median_time_context, 4, 2);
    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::InvalidSince)
    );
}
