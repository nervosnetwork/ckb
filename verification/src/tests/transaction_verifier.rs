use super::super::transaction_verifier::{
    CapacityVerifier, DuplicateDepsVerifier, EmptyVerifier, MaturityVerifier, Since, SinceVerifier,
    SizeVerifier, VersionVerifier,
};
use crate::error::TransactionError;
use ckb_core::cell::{BlockInfo, CellMeta, CellMetaBuilder, ResolvedOutPoint, ResolvedTransaction};
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, Transaction, TransactionBuilder};
use ckb_core::{capacity_bytes, BlockNumber, Bytes, Capacity, Version};
use ckb_db::MemoryKeyValueDB;
use ckb_resource::CODE_HASH_DAO;
use ckb_store::{ChainKVStore, COLUMNS};
use ckb_traits::BlockMedianTimeContext;
use numext_fixed_hash::{h256, H256};
use std::sync::Arc;

fn new_memory_store() -> Arc<ChainKVStore<MemoryKeyValueDB>> {
    Arc::new(ChainKVStore::new(MemoryKeyValueDB::open(COLUMNS as usize)))
}

#[test]
pub fn test_empty() {
    let transaction = TransactionBuilder::default().build();
    let verifier = EmptyVerifier::new(&transaction);

    assert_eq!(verifier.verify().err(), Some(TransactionError::Empty));
}

#[test]
pub fn test_version() {
    let transaction = TransactionBuilder::default()
        .version(Version::default() + 1)
        .build();
    let verifier = VersionVerifier::new(&transaction);

    assert_eq!(verifier.verify().err(), Some(TransactionError::Version));
}

#[test]
pub fn test_exceeded_maximum_block_bytes() {
    let transaction = TransactionBuilder::default()
        .version(Version::default() + 1)
        .output(CellOutput::new(
            capacity_bytes!(50),
            vec![1; 500].into(),
            Script::default(),
            None,
        ))
        .build();
    let verifier = SizeVerifier::new(&transaction, 100);

    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::ExceededMaximumBlockBytes)
    );
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
    let store = new_memory_store();
    let verifier = CapacityVerifier::new(&rtx, &store);

    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::InsufficientCellCapacity)
    );
}

#[test]
pub fn test_skip_dao_capacity_check() {
    let transaction = TransactionBuilder::default()
        .output(CellOutput::new(
            capacity_bytes!(500),
            Bytes::from(vec![1; 10]),
            Script::default(),
            Some(Script::new(vec![], CODE_HASH_DAO)),
        ))
        .build();

    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_deps: Vec::new(),
        resolved_inputs: vec![],
    };
    let store = new_memory_store();
    let verifier = CapacityVerifier::new(&rtx, &store);

    assert!(verifier.verify().is_ok());
}

// inputs immature verify
#[test]
pub fn test_inputs_cellbase_maturity() {
    let transaction = TransactionBuilder::default().build();
    let output = CellOutput::new(capacity_bytes!(50), Bytes::new(), Script::default(), None);

    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_deps: Vec::new(),
        resolved_inputs: vec![ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.clone())
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

// deps immature verify
#[test]
pub fn test_deps_cellbase_maturity() {
    let transaction = TransactionBuilder::default().build();
    let output = CellOutput::new(capacity_bytes!(50), Bytes::new(), Script::default(), None);

    // The 1st dep is cellbase, the 2nd one is not.
    let rtx = ResolvedTransaction {
        transaction: &transaction,
        resolved_deps: vec![
            ResolvedOutPoint::cell_only(
                CellMetaBuilder::from_cell_output(output.clone())
                    .block_info(BlockInfo::new(30, 0))
                    .cellbase(true)
                    .build(),
            ),
            ResolvedOutPoint::cell_only(
                CellMetaBuilder::from_cell_output(output.clone())
                    .block_info(BlockInfo::new(40, 0))
                    .cellbase(false)
                    .build(),
            ),
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

    // The inputs capacity is 49 + 100 = 149,
    // is less than outputs capacity
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
    let store = new_memory_store();
    let verifier = CapacityVerifier::new(&rtx, &store);

    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::OutputsSumOverflow)
    );
}

#[test]
pub fn test_duplicate_deps() {
    let out_point = OutPoint::new_cell(h256!("0x1"), 0);
    let transaction = TransactionBuilder::default()
        .deps(vec![out_point.clone(), out_point])
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
        .inputs(vec![CellInput::new(
            OutPoint::new_cell(h256!("0x1"), 0),
            since,
        )])
        .build()
}

fn create_resolve_tx_with_block_info(
    tx: &Transaction,
    block_info: BlockInfo,
) -> ResolvedTransaction<'_> {
    ResolvedTransaction {
        transaction: &tx,
        resolved_deps: Vec::new(),
        resolved_inputs: vec![ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(CellOutput::new(
                capacity_bytes!(50),
                Bytes::new(),
                Script::default(),
                None,
            ))
            .block_info(block_info)
            .build(),
        )],
    }
}

#[test]
fn test_invalid_since_verify() {
    // use remain flags
    let tx = create_tx_with_lock(0x0100_0000_0000_0001);
    let rtx = create_resolve_tx_with_block_info(&tx, BlockInfo::new(1, 0));

    let median_time_context = FakeMedianTime {
        timestamps: vec![0; 11],
    };
    let verifier = SinceVerifier::new(&rtx, &median_time_context, 5, 1);
    assert_eq!(
        verifier.verify().err(),
        Some(TransactionError::InvalidSince)
    );
}

#[test]
pub fn test_absolute_block_number_lock() {
    // absolute lock until block number 0xa
    let tx = create_tx_with_lock(0x0000_0000_0000_000a);
    let rtx = create_resolve_tx_with_block_info(&tx, BlockInfo::new(1, 0));

    let median_time_context = FakeMedianTime {
        timestamps: vec![0; 11],
    };
    let verifier = SinceVerifier::new(&rtx, &median_time_context, 5, 1);
    assert_eq!(verifier.verify().err(), Some(TransactionError::Immature));
    // spent after 10 height
    let verifier = SinceVerifier::new(&rtx, &median_time_context, 10, 1);
    assert!(verifier.verify().is_ok());
}

#[test]
pub fn test_absolute_epoch_number_lock() {
    // absolute lock until epoch number 0xa
    let tx = create_tx_with_lock(0x2000_0000_0000_000a);
    let rtx = create_resolve_tx_with_block_info(&tx, BlockInfo::new(1, 0));

    let median_time_context = FakeMedianTime {
        timestamps: vec![0; 11],
    };
    let verifier = SinceVerifier::new(&rtx, &median_time_context, 5, 1);
    assert_eq!(verifier.verify().err(), Some(TransactionError::Immature));
    // spent after 10 epoch
    let verifier = SinceVerifier::new(&rtx, &median_time_context, 100, 10);
    assert!(verifier.verify().is_ok());
}

#[test]
pub fn test_relative_timestamp_lock() {
    // relative lock timestamp lock
    let tx = create_tx_with_lock(0xc000_0000_0000_0002);
    let rtx = create_resolve_tx_with_block_info(&tx, BlockInfo::new(1, 0));

    let median_time_context = FakeMedianTime {
        timestamps: vec![0; 11],
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
}

#[test]
pub fn test_relative_epoch() {
    // next epoch
    let tx = create_tx_with_lock(0xa000_0000_0000_0001);
    let rtx = create_resolve_tx_with_block_info(&tx, BlockInfo::new(1, 1));

    let median_time_context = FakeMedianTime {
        timestamps: vec![0; 11],
    };

    let verifier = SinceVerifier::new(&rtx, &median_time_context, 4, 1);
    assert_eq!(verifier.verify().err(), Some(TransactionError::Immature));

    let verifier = SinceVerifier::new(&rtx, &median_time_context, 4, 2);
    assert!(verifier.verify().is_ok());
}

#[test]

pub fn test_since_both() {
    // both
    let transaction = TransactionBuilder::default()
        .inputs(vec![
            // absolute lock until epoch number 0xa
            CellInput::new(OutPoint::new_cell(h256!("0x1"), 0), 0x0000_0000_0000_000a),
            // relative lock until after 2 blocks
            CellInput::new(OutPoint::new_cell(h256!("0x1"), 0), 0xc000_0000_0000_0002),
        ])
        .build();

    let rtx = create_resolve_tx_with_block_info(&transaction, BlockInfo::new(1, 0));
    // spent after 1024 seconds and 4 blocks (less than 10 blocks)
    // fake median time: 1124
    let median_time_context = FakeMedianTime {
        timestamps: vec![0, 100_000, 1_124_000, 2_000_000, 3_000_000],
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
}
