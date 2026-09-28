use super::super::transaction_verifier::{
    CapacityVerifier, DaoScriptSizeVerifier, DuplicateDepsVerifier, EmptyVerifier,
    MaturityVerifier, OutputsDataVerifier, Since, SinceVerifier, SizeVerifier, VersionVerifier,
};
use crate::cache::{
    ScriptVerificationOutcome, ScriptVerificationProof, ScriptVerificationRules,
    TxVerificationCacheKey,
};
use crate::error::TransactionErrorSource;
use crate::transaction_verifier::ScriptHashTypeVerifier;
use crate::{
    ContextualTransactionVerifier, ScriptError, TimeRelativeTransactionVerifier, TransactionError,
    TxVerifyEnv, transaction_depends_on_time,
};
use ckb_chain_spec::{
    OUTPUT_INDEX_DAO, build_genesis_type_id_script,
    consensus::{Consensus, ConsensusBuilder},
};
use ckb_error::{Error, assert_error_eq};
use ckb_test_chain_utils::{MOCK_MEDIAN_TIME_COUNT, MockMedianTime};
use ckb_traits::{
    CellDataProvider, EpochProvider, ExtensionProvider, HeaderFields, HeaderFieldsProvider,
    HeaderProvider,
};
use ckb_types::{
    bytes::Bytes,
    constants::TX_VERSION,
    core::{
        BlockExt, BlockNumber, Capacity, Cycle, EpochExt, EpochNumber, EpochNumberWithFraction,
        HeaderView, ScriptHashType, TransactionBuilder, TransactionInfo, TransactionView,
        capacity_bytes,
        cell::{CellMeta, CellMetaBuilder, ResolvedTransaction},
        hardfork::HardForks,
    },
    h256,
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint, Script},
    prelude::*,
};
use std::{cell::Cell, sync::Arc};

fn time_fixture(since: u64) -> ResolvedTransaction {
    ResolvedTransaction::dummy_resolve(
        TransactionBuilder::default()
            .input(CellInput::new(
                OutPoint::new(Byte32::new([80; 32]), 0),
                since,
            ))
            .cell_dep(
                CellDep::new_builder()
                    .out_point(OutPoint::new(Byte32::new([81; 32]), 0))
                    .build(),
            )
            .cell_dep(
                CellDep::new_builder()
                    .out_point(OutPoint::new(Byte32::new([82; 32]), 0))
                    .dep_type(ckb_types::core::DepType::DepGroup)
                    .build(),
            )
            .build(),
    )
}

fn verify_time_at(
    resolved: Arc<ResolvedTransaction>,
    data_loader: &MockMedianTime,
    block_number: u64,
    epoch_number: u64,
) -> Result<(), Error> {
    let consensus = Arc::new(
        ConsensusBuilder::default()
            .median_time_block_count(11)
            .cellbase_maturity(EpochNumberWithFraction::new(2, 0, 1))
            .build(),
    );
    let header = HeaderView::new_advanced_builder()
        .number(block_number)
        .epoch(EpochNumberWithFraction::new(epoch_number, 0, 1))
        .parent_hash(data_loader.get_block_hash(block_number - 1))
        .build();
    TimeRelativeTransactionVerifier::new(
        resolved,
        consensus,
        data_loader.clone(),
        Arc::new(TxVerifyEnv::new_commit(&header)),
    )
    .verify()
}

#[test]
fn time_dependency_matches_canonical_cellbase_applicability() {
    let data_loader = MockMedianTime::new(vec![0; 11]);
    for role in ["input", "cell-dep", "dep-group"] {
        for (location, needs_maturity) in [
            (None, false),
            (Some((0, 0)), false), // Genesis cellbase is exempt.
            (Some((1, 1)), false), // Ordinary transactions are exempt.
            (Some((1, 0)), true),
        ] {
            let mut resolved = time_fixture(0);
            let cells = match role {
                "input" => &mut resolved.resolved_inputs,
                "cell-dep" => &mut resolved.resolved_cell_deps,
                "dep-group" => &mut resolved.resolved_dep_groups,
                _ => unreachable!(),
            };
            cells[0].transaction_info = location.map(|(block, index)| {
                data_loader.get_transaction_info(
                    block,
                    EpochNumberWithFraction::new(1, 0, 1),
                    index,
                )
            });
            // Direct code deps and expanded group members share the cell-dep
            // role. A group container has no canonical maturity condition.
            let expected = needs_maturity && role != "dep-group";
            assert_eq!(transaction_depends_on_time(&resolved), expected, "{role}");
            let resolved = Arc::new(resolved);
            assert_eq!(
                verify_time_at(Arc::clone(&resolved), &data_loader, 5, 2).is_err(),
                expected,
                "{role} before the maturity boundary"
            );
            assert!(verify_time_at(resolved, &data_loader, 5, 3).is_ok());
        }
    }
}

#[test]
fn time_dependency_matches_all_since_metrics_and_forms() {
    let data_loader = MockMedianTime::new(vec![
        0, 0, 0, 0, 5_000, 5_000, 5_000, 5_000, 5_000, 5_000, 5_000,
    ]);
    for since in [
        11,                    // Absolute block number.
        0x8000_0000_0000_000a, // Relative block number: 1 + 10.
        0x2000_0100_0000_0003, // Absolute epoch 3.
        0xa000_0100_0000_0002, // Relative epoch: 1 + 2.
        0x4000_0000_0000_0005, // Absolute median time: 5 seconds.
        0xc000_0000_0000_0005, // Relative median time: 0 + 5 seconds.
    ] {
        let mut resolved = time_fixture(since);
        resolved.resolved_inputs[0].transaction_info =
            Some(data_loader.get_transaction_info(1, EpochNumberWithFraction::new(1, 0, 1), 1));
        assert!(transaction_depends_on_time(&resolved), "since {since:#x}");
        let resolved = Arc::new(resolved);
        assert_error_eq!(
            verify_time_at(Arc::clone(&resolved), &data_loader, 4, 2).unwrap_err(),
            TransactionError::Immature { index: 0 }
        );
        assert!(verify_time_at(resolved, &data_loader, 11, 3).is_ok());
    }
}

#[test]
fn time_candidates_preserve_error_order_source_and_original_index() {
    let data_loader = MockMedianTime::new(vec![0; 11]);
    let mut resolved = time_fixture(0);
    let mut immature = resolved.resolved_inputs[0].clone();
    immature.transaction_info =
        Some(data_loader.get_transaction_info(1, EpochNumberWithFraction::new(1, 0, 1), 0));
    resolved
        .resolved_inputs
        .extend([immature.clone(), immature.clone()]);
    resolved.resolved_cell_deps.push(immature);
    resolved.transaction = resolved
        .transaction
        .as_advanced_builder()
        .input(CellInput::new(OutPoint::new(Byte32::new([84; 32]), 0), 0))
        .input(CellInput::new(
            OutPoint::new(Byte32::new([85; 32]), 0),
            0x0100_0000_0000_0001,
        ))
        .build();

    for index in [1, 2] {
        assert_error_eq!(
            verify_time_at(Arc::new(resolved.clone()), &data_loader, 5, 2).unwrap_err(),
            TransactionError::CellbaseImmaturity {
                inner: TransactionErrorSource::Inputs,
                index
            }
        );
        resolved.resolved_inputs[index].transaction_info = None;
    }
    assert_error_eq!(
        verify_time_at(Arc::new(resolved.clone()), &data_loader, 5, 2).unwrap_err(),
        TransactionError::CellbaseImmaturity {
            inner: TransactionErrorSource::CellDeps,
            index: 1
        }
    );
    resolved.resolved_cell_deps[1].transaction_info = None;
    assert!(transaction_depends_on_time(&resolved));
    assert_error_eq!(
        verify_time_at(Arc::new(resolved), &data_loader, 5, 2).unwrap_err(),
        TransactionError::InvalidSince { index: 2 }
    );
}

#[test]
pub fn test_empty() {
    let transaction = TransactionBuilder::default().build();
    let verifier = EmptyVerifier::new(&transaction);

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        TransactionError::Empty {
            inner: TransactionErrorSource::Inputs,
        }
    );
}

#[test]
pub fn test_version() {
    let transaction = TransactionBuilder::default()
        .version(TX_VERSION + 1)
        .build();
    let verifier = VersionVerifier::new(&transaction, TX_VERSION);

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        TransactionError::MismatchedVersion {
            expected: 0,
            actual: 1
        },
    );
}

#[test]
pub fn test_exceeded_maximum_block_bytes() {
    let data: Bytes = vec![1; 500].into();
    let transaction = TransactionBuilder::default()
        .output(
            CellOutput::new_builder()
                .capacity(capacity_bytes!(50))
                .build(),
        )
        .output_data(data)
        .build();
    let verifier = SizeVerifier::new(&transaction, 100);

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        TransactionError::ExceededMaximumBlockBytes {
            actual: 661,
            limit: 100
        },
    );
}

#[test]
pub fn test_unknown_hash_type_output_lock() {
    let transaction = TransactionBuilder::default()
        .output(
            CellOutput::new_builder()
                .lock(Script::default().as_builder().hash_type(3).build())
                .build(),
        )
        .build();
    let verifier = ScriptHashTypeVerifier::new(&transaction);

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        TransactionError::InvalidScriptHashType {
            hash_type: 3.into(),
        },
    );
}

#[test]
pub fn test_not_enabled_hash_type_output_lock() {
    let transaction = TransactionBuilder::default()
        .output(
            CellOutput::new_builder()
                .lock(
                    Script::default()
                        .as_builder()
                        .hash_type(ScriptHashType::Data3)
                        .build(),
                )
                .build(),
        )
        .build();
    let verifier = ScriptHashTypeVerifier::new(&transaction);

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        TransactionError::ScriptHashTypeNotPermitted {
            hash_type: ScriptHashType::Data3.into(),
        },
    );
}

#[test]
pub fn test_capacity_out_of_bound() {
    let data = Bytes::from(vec![1; 51]);
    let transaction = TransactionBuilder::default()
        .output(
            CellOutput::new_builder()
                .capacity(capacity_bytes!(50))
                .build(),
        )
        .output_data(data)
        .build();

    let rtx = Arc::new(ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(50))
                    .build(),
                Bytes::new(),
            )
            .build(),
        ],
        resolved_dep_groups: vec![],
    });
    let dao_type_hash = build_genesis_type_id_script(OUTPUT_INDEX_DAO).calc_script_hash();
    let verifier = CapacityVerifier::new(rtx, dao_type_hash);

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        TransactionError::InsufficientCellCapacity {
            inner: TransactionErrorSource::Outputs,
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
                .capacity(capacity_bytes!(500))
                .type_(Some(dao_type_script.clone()))
                .build(),
        )
        .output_data(Bytes::new())
        .build();

    let rtx = Arc::new(ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![],
        resolved_dep_groups: vec![],
    });
    let verifier = CapacityVerifier::new(rtx, dao_type_script.calc_script_hash());

    assert!(verifier.verify().is_ok());
}

// inputs immature verify
#[test]
pub fn test_inputs_cellbase_maturity() {
    let transaction = TransactionBuilder::default().build();
    let output = CellOutput::new_builder()
        .capacity(capacity_bytes!(50))
        .build();
    let base_epoch = EpochNumberWithFraction::new(10, 0, 10);
    let cellbase_maturity = EpochNumberWithFraction::new(5, 0, 1);

    let rtx = Arc::new(ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_dep_groups: Vec::new(),
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(output, Bytes::new())
                .transaction_info(mock_transaction_info(30, base_epoch, 0))
                .build(),
        ],
    });

    let mut current_epoch = EpochNumberWithFraction::new(0, 0, 10);
    let threshold = cellbase_maturity.to_rational() + base_epoch.to_rational();
    while current_epoch.number() < cellbase_maturity.number() + base_epoch.number() + 5 {
        let verifier = MaturityVerifier::new(Arc::clone(&rtx), current_epoch, cellbase_maturity);
        let current = current_epoch.to_rational();
        if current < threshold {
            assert_error_eq!(
                verifier.verify().unwrap_err(),
                TransactionError::CellbaseImmaturity {
                    inner: TransactionErrorSource::Inputs,
                    index: 0
                },
                "base_epoch = {base_epoch}, current_epoch = {current_epoch}, cellbase_maturity = {cellbase_maturity}"
            );
        } else {
            assert!(
                verifier.verify().is_ok(),
                "base_epoch = {base_epoch}, current_epoch = {current_epoch}, cellbase_maturity = {cellbase_maturity}"
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
        .capacity(capacity_bytes!(50))
        .build();
    let base_epoch = EpochNumberWithFraction::new(0, 0, 10);
    let cellbase_maturity = EpochNumberWithFraction::new(5, 0, 1);
    // Transaction use genesis cellbase
    let rtx = Arc::new(ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_dep_groups: Vec::new(),
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(output, Bytes::new())
                .transaction_info(mock_transaction_info(0, base_epoch, 0))
                .build(),
        ],
    });

    let mut current_epoch = EpochNumberWithFraction::new(0, 0, 10);
    while current_epoch.number() < cellbase_maturity.number() + base_epoch.number() + 5 {
        let verifier = MaturityVerifier::new(Arc::clone(&rtx), current_epoch, cellbase_maturity);
        assert!(
            verifier.verify().is_ok(),
            "base_epoch = {base_epoch}, current_epoch = {current_epoch}, cellbase_maturity = {cellbase_maturity}"
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
        .capacity(capacity_bytes!(50))
        .build();

    let base_epoch = EpochNumberWithFraction::new(0, 0, 10);
    let cellbase_maturity = EpochNumberWithFraction::new(5, 0, 1);

    // The 1st dep is cellbase, the 2nd one is not.
    let rtx = Arc::new(ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![
            CellMetaBuilder::from_cell_output(output.clone(), Bytes::new())
                .transaction_info(mock_transaction_info(30, base_epoch, 0))
                .build(),
            CellMetaBuilder::from_cell_output(output, Bytes::new())
                .transaction_info(mock_transaction_info(40, base_epoch, 1))
                .build(),
        ],
        resolved_inputs: Vec::new(),
        resolved_dep_groups: vec![],
    });

    let mut current_epoch = EpochNumberWithFraction::new(0, 0, 10);
    let threshold = cellbase_maturity.to_rational() + base_epoch.to_rational();
    while current_epoch.number() < cellbase_maturity.number() + base_epoch.number() + 5 {
        let verifier = MaturityVerifier::new(Arc::clone(&rtx), current_epoch, cellbase_maturity);
        let current = current_epoch.to_rational();
        if current < threshold {
            assert_error_eq!(
                verifier.verify().unwrap_err(),
                TransactionError::CellbaseImmaturity {
                    inner: TransactionErrorSource::CellDeps,
                    index: 0
                },
                "base_epoch = {base_epoch}, current_epoch = {current_epoch}, cellbase_maturity = {cellbase_maturity}"
            );
        } else {
            assert!(
                verifier.verify().is_ok(),
                "base_epoch = {base_epoch}, current_epoch = {current_epoch}, cellbase_maturity = {cellbase_maturity}"
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
                .capacity(capacity_bytes!(50))
                .build(),
            CellOutput::new_builder()
                .capacity(capacity_bytes!(100))
                .build(),
        ])
        .outputs_data(vec![Bytes::new().into(); 2])
        .build();

    // The inputs capacity is 49 + 100 = 149,
    // is less than outputs capacity
    let rtx = Arc::new(ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(49))
                    .build(),
                Bytes::new(),
            )
            .build(),
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(100))
                    .build(),
                Bytes::new(),
            )
            .build(),
        ],
        resolved_dep_groups: vec![],
    });
    let dao_type_hash = build_genesis_type_id_script(OUTPUT_INDEX_DAO).calc_script_hash();
    let verifier = CapacityVerifier::new(rtx, dao_type_hash);

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        TransactionError::OutputsSumOverflow {
            inputs_sum: capacity_bytes!(149),
            outputs_sum: capacity_bytes!(150),
        },
    );
}

#[derive(Clone, Default)]
struct ContextualTestDataLoader {
    header: Option<HeaderView>,
}

impl CellDataProvider for ContextualTestDataLoader {
    fn get_cell_data(&self, _out_point: &OutPoint) -> Option<Bytes> {
        None
    }

    fn get_cell_data_hash(&self, _out_point: &OutPoint) -> Option<Byte32> {
        None
    }
}

impl HeaderProvider for ContextualTestDataLoader {
    fn get_header(&self, hash: &Byte32) -> Option<HeaderView> {
        self.header
            .as_ref()
            .filter(|header| header.hash() == *hash)
            .cloned()
    }
}

impl ExtensionProvider for ContextualTestDataLoader {
    fn get_block_extension(&self, hash: &Byte32) -> Option<ckb_types::packed::Bytes> {
        self.header
            .as_ref()
            .filter(|header| header.hash() == *hash)
            .map(|_| Bytes::from_static(b"extension").pack())
    }
}

impl HeaderFieldsProvider for ContextualTestDataLoader {
    fn get_header_fields(&self, _hash: &Byte32) -> Option<HeaderFields> {
        None
    }
}

impl EpochProvider for ContextualTestDataLoader {
    fn get_epoch_ext(&self, _block_header: &HeaderView) -> Option<EpochExt> {
        None
    }

    fn get_block_hash(&self, _number: BlockNumber) -> Option<Byte32> {
        None
    }

    fn get_block_ext(&self, _block_hash: &Byte32) -> Option<BlockExt> {
        None
    }

    fn get_block_header(&self, _hash: &Byte32) -> Option<HeaderView> {
        None
    }
}

fn contextual_verifier_with_sealed_script_proof(
    input_capacity: Capacity,
    output_capacity: Capacity,
    cached_script_cycles: Cycle,
    since: u64,
) -> (
    ContextualTransactionVerifier<ContextualTestDataLoader>,
    ScriptVerificationProof,
) {
    let transaction = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(Byte32::zero(), 0), since))
        .output(CellOutput::new_builder().capacity(output_capacity).build())
        .output_data(Bytes::new())
        .build();
    let rtx = Arc::new(ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder().capacity(input_capacity).build(),
                Bytes::new(),
            )
            .build(),
        ],
        resolved_dep_groups: Vec::new(),
    });
    let consensus = Arc::new(ConsensusBuilder::default().build());
    let tx_env = Arc::new(TxVerifyEnv::new_commit(
        &HeaderView::new_advanced_builder().build(),
    ));
    let rules = ScriptVerificationRules::from_env(&consensus, &tx_env);
    let key = TxVerificationCacheKey::from_resolved(&rtx, rules);
    let proof = ScriptVerificationProof::from_vm_success(key, cached_script_cycles);
    (
        ContextualTransactionVerifier::new(
            rtx,
            consensus,
            ContextualTestDataLoader::default(),
            tx_env,
        ),
        proof,
    )
}

#[test]
fn canonical_verifier_reexecutes_when_a_cached_cells_visible_origin_changes() {
    for (hash_type, syscall) in [
        (ScriptHashType::Data, 2072u16),  // LOAD_HEADER in VM0
        (ScriptHashType::Data2, 2104u16), // LOAD_BLOCK_EXTENSION in VM2
    ] {
        for source in [1u8, 3u8] {
            // SOURCE_INPUT and SOURCE_CELL_DEP
            check_cached_cell_origin(hash_type, syscall, source);
        }
    }
}

fn check_cached_cell_origin(hash_type: ScriptHashType, syscall: u16, source: u8) {
    let program = Bytes::from_static(include_bytes!("../../../script/testdata/load_cell_origin"));
    let script = Script::new_builder()
        .code_hash(CellOutput::calc_data_hash(&program))
        .hash_type(hash_type)
        .build();
    let origin = HeaderView::new_advanced_builder()
        .number(1u64)
        .epoch(EpochNumberWithFraction::new(0, 1, 10))
        .build();
    let transaction = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(h256!("0x1").into(), 0), 0))
        .cell_dep(
            CellDep::new_builder()
                .out_point(OutPoint::new(h256!("0x2").into(), 0))
                .build(),
        )
        .header_dep(origin.hash())
        .output(
            CellOutput::new_builder()
                .capacity(capacity_bytes!(100))
                .build(),
        )
        .output_data(Bytes::new())
        .build();
    let mut resolved = ResolvedTransaction {
        transaction,
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(200))
                    .lock(script)
                    .build(),
                Bytes::new(),
            )
            .build(),
        ],
        resolved_cell_deps: vec![
            CellMetaBuilder::from_cell_output(CellOutput::default(), program).build(),
        ],
        resolved_dep_groups: Vec::new(),
    };
    let consensus = Arc::new(
        ConsensusBuilder::default()
            .hardfork_switch(HardForks::new_dev())
            .build(),
    );
    let env = Arc::new(TxVerifyEnv::new_commit(&origin));
    let verifier = |rtx: &ResolvedTransaction| {
        ContextualTransactionVerifier::new(
            Arc::new(rtx.clone()),
            Arc::clone(&consensus),
            ContextualTestDataLoader {
                header: Some(origin.clone()),
            },
            Arc::clone(&env),
        )
    };
    let confirmed = TransactionInfo {
        block_hash: origin.hash(),
        block_number: 1,
        block_epoch: origin.epoch(),
        index: 1,
    };
    for starts_confirmed in [false, true] {
        // The script requires the starting visibility. The transaction and its
        // witness stay identical when the cell confirms or is detached below.
        let expected_status = if starts_confirmed { 0 } else { 2 };
        let [syscall_low, syscall_high] = syscall.to_le_bytes();
        resolved.transaction = resolved
            .transaction
            .as_advanced_builder()
            .set_witnesses(vec![
                Bytes::from(vec![expected_status, source, syscall_low, syscall_high]).pack(),
            ])
            .build();
        let cell = if source == 1 {
            &mut resolved.resolved_inputs[0]
        } else {
            &mut resolved.resolved_cell_deps[0]
        };
        cell.transaction_info = starts_confirmed.then(|| confirmed.clone());
        let proof = verifier(&resolved)
            .verify_scripts(u64::MAX, None)
            .unwrap()
            .executed_proof()
            .unwrap();
        assert!(
            verifier(&resolved)
                .verify_scripts(u64::MAX, Some(proof))
                .unwrap()
                .was_reused()
        );

        let cell = if source == 1 {
            &mut resolved.resolved_inputs[0]
        } else {
            &mut resolved.resolved_cell_deps[0]
        };
        cell.transaction_info = (!starts_confirmed).then(|| confirmed.clone());
        let error = verifier(&resolved)
            .verify_scripts(u64::MAX, Some(proof))
            .unwrap_err();
        assert_error_eq!(
            error,
            ScriptError::validation_failure(&resolved.resolved_inputs[0].cell_output.lock(), 42)
                .input_lock_script(0),
            "a proof cannot hide a changed origin syscall result after confirmation or reorg"
        );
    }
}

#[test]
fn test_sealed_script_proof_does_not_skip_capacity_verification() {
    let (verifier, proof) = contextual_verifier_with_sealed_script_proof(
        capacity_bytes!(149),
        capacity_bytes!(150),
        42,
        0,
    );

    assert_error_eq!(
        verifier.verify_block(u64::MAX, Some(proof)).unwrap_err(),
        TransactionError::OutputsSumOverflow {
            inputs_sum: capacity_bytes!(149),
            outputs_sum: capacity_bytes!(150),
        },
    );
}

#[test]
fn test_sealed_script_proof_recalculates_fee() {
    let (verifier, proof) = contextual_verifier_with_sealed_script_proof(
        capacity_bytes!(200),
        capacity_bytes!(150),
        42,
        0,
    );

    let (outcome, fee) = verifier
        .verify_block(u64::MAX, Some(proof))
        .expect("the exact sealed script proof remains reusable");
    assert!(matches!(outcome, ScriptVerificationOutcome::Reused(_)));
    assert_eq!(outcome.cycles(), 42);
    assert_eq!(fee, capacity_bytes!(50));
}

#[test]
fn test_sealed_script_proof_respects_max_cycles() {
    let (verifier, proof) = contextual_verifier_with_sealed_script_proof(
        capacity_bytes!(200),
        capacity_bytes!(150),
        42,
        0,
    );

    assert_error_eq!(
        verifier.verify_block(41, Some(proof)).unwrap_err(),
        ScriptError::ExceededMaximumCycles(41).unknown_source(),
    );
}

#[cfg(not(target_family = "wasm"))]
#[tokio::test]
async fn test_contextual_entry_points_preserve_error_and_cache_priority() {
    use ckb_script::{SchedulerRunner, types::TerminatedResult};

    struct UnexpectedVm;

    impl<S: Send> SchedulerRunner<S> for UnexpectedVm {
        async fn run(
            &mut self,
            _scheduler: S,
            _max_cycles: Cycle,
        ) -> Result<Option<TerminatedResult>, ckb_vm::Error> {
            panic!("context checks and reusable proofs must not invoke the VM");
        }
    }

    let cases = [
        (
            1,
            capacity_bytes!(149),
            41,
            Some(Error::from(TransactionError::Immature { index: 0 })),
        ),
        (
            0,
            capacity_bytes!(149),
            41,
            Some(Error::from(TransactionError::OutputsSumOverflow {
                inputs_sum: capacity_bytes!(149),
                outputs_sum: capacity_bytes!(150),
            })),
        ),
        (
            0,
            capacity_bytes!(200),
            41,
            Some(Error::from(
                ScriptError::ExceededMaximumCycles(41).unknown_source(),
            )),
        ),
        (0, capacity_bytes!(200), 42, None),
    ];
    for (since, input_capacity, max_cycles, expected) in cases {
        let (verifier, proof) = contextual_verifier_with_sealed_script_proof(
            input_capacity,
            capacity_bytes!(150),
            42,
            since,
        );
        // Context errors and reusable proofs must bypass the VM runner.
        let results = [
            verifier.verify_scripts(max_cycles, Some(proof)),
            verifier
                .verify_block(max_cycles, Some(proof))
                .map(|(outcome, _)| outcome),
            verifier
                .verify_with_runner(max_cycles, Some(proof), &mut UnexpectedVm)
                .await
                .map(|outcome| outcome.expect("a reusable proof completes verification")),
        ];
        for result in results {
            if let Some(error) = &expected {
                assert_eq!(result.unwrap_err().to_string(), error.to_string());
            } else {
                let outcome = result.unwrap();
                assert!(matches!(outcome, ScriptVerificationOutcome::Reused(_)));
                assert_eq!(outcome.cycles(), 42);
            }
        }
    }
}

#[test]
pub fn test_duplicate_cell_deps() {
    let out_point = OutPoint::new(h256!("0x1").into(), 0);
    let cell_dep = CellDep::new_builder().out_point(out_point).build();
    let transaction = TransactionBuilder::default()
        .cell_deps(vec![cell_dep.clone(), cell_dep.clone()])
        .build();

    let verifier = DuplicateDepsVerifier::new(&transaction);

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        TransactionError::DuplicateCellDeps {
            out_point: cell_dep.out_point()
        },
    );
}

#[test]
pub fn test_duplicate_header_deps() {
    let transaction = TransactionBuilder::default()
        .header_deps(vec![h256!("0x1").into(), h256!("0x1").into()])
        .build();

    let verifier = DuplicateDepsVerifier::new(&transaction);

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        TransactionError::DuplicateHeaderDeps {
            hash: h256!("0x1").into()
        },
    );
}

fn verify_since(
    rtx: Arc<ResolvedTransaction>,
    median_time_context: MockMedianTime,
    block_number: BlockNumber,
    epoch_number: EpochNumber,
) -> Result<(), Error> {
    let parent_hash = Arc::new(median_time_context.get_last_block_hash());
    let consensus = Arc::new(
        ConsensusBuilder::default()
            .median_time_block_count(11)
            .build(),
    );
    let tx_env = {
        let epoch = EpochNumberWithFraction::new(epoch_number, 0, 10);
        let header = HeaderView::new_advanced_builder()
            .number(block_number)
            .epoch(epoch)
            .parent_hash(parent_hash.as_ref().to_owned())
            .build();
        Arc::new(TxVerifyEnv::new_commit(&header))
    };
    SinceVerifier::new(rtx, consensus, median_time_context, tx_env).verify()
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
        assert!(since.flags_is_valid());
    }

    let invalids = vec![
        0x0100_0000_0000_0001,
        0x1000_0000_0000_0001,
        0xd000_0000_0000_0001,
    ];

    for v in invalids.into_iter() {
        let since = Since(v);
        assert!(!since.flags_is_valid());
    }
}

fn create_tx_with_lock(since: u64) -> TransactionView {
    TransactionBuilder::default()
        .inputs(vec![CellInput::new(
            OutPoint::new(h256!("0x1").into(), 0),
            since,
        )])
        .build()
}

fn create_resolve_tx_with_transaction_info(
    tx: &TransactionView,
    transaction_info: TransactionInfo,
) -> Arc<ResolvedTransaction> {
    Arc::new(ResolvedTransaction {
        transaction: tx.clone(),
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(50))
                    .build(),
                Bytes::new(),
            )
            .transaction_info(transaction_info)
            .build(),
        ],
        resolved_dep_groups: vec![],
    })
}

#[test]
fn test_invalid_since_verify() {
    // use remain flags
    let tx = create_tx_with_lock(0x0100_0000_0000_0001);
    let median_time_context = MockMedianTime::new(vec![0; 11]);
    let rtx = create_resolve_tx_with_transaction_info(
        &tx,
        median_time_context.get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
    );

    assert_error_eq!(
        verify_since(rtx, median_time_context, 5, 1).unwrap_err(),
        TransactionError::InvalidSince { index: 0 },
    );
}

#[test]
fn test_timestamp_since_millis_overflow_is_invalid() {
    for flag in &[
        0x4000_0000_0000_0000u64, // absolute & time
        0xc000_0000_0000_0000u64, // relative & time
    ] {
        let tx = create_tx_with_lock(*flag | (u64::MAX / 1000 + 1));
        let median_time_context = MockMedianTime::new(vec![0; 11]);
        let rtx = create_resolve_tx_with_transaction_info(
            &tx,
            median_time_context.get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
        );

        assert_error_eq!(
            verify_since(rtx, median_time_context, 5, 1).unwrap_err(),
            TransactionError::InvalidSince { index: 0 },
        );
    }
}

#[test]
fn test_valid_zero_length_since() {
    // use remain flags
    let tx = create_tx_with_lock(0xa000_0000_0000_0000);
    let median_time_context = MockMedianTime::new(vec![0; 11]);
    let rtx = create_resolve_tx_with_transaction_info(
        &tx,
        median_time_context.get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
    );

    assert!(verify_since(rtx, median_time_context, 5, 1).is_ok(),);
}

#[test]
fn test_fraction_epoch_since_verify() {
    let tx = create_tx_with_lock(0x2000_0a00_0500_0010);
    let median_time_context = MockMedianTime::new(vec![0; 11]);
    let rtx = create_resolve_tx_with_transaction_info(
        &tx,
        median_time_context.get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
    );
    let consensus = Arc::new(
        ConsensusBuilder::default()
            .median_time_block_count(MOCK_MEDIAN_TIME_COUNT)
            .build(),
    );

    let block_number = 11;
    let parent_hash = Arc::new(median_time_context.get_block_hash(block_number - 1));

    let tx_env = {
        let epoch = EpochNumberWithFraction::new(16, 1, 10);
        let header = HeaderView::new_advanced_builder()
            .number(block_number)
            .epoch(epoch)
            .parent_hash(parent_hash.as_ref().to_owned())
            .build();
        Arc::new(TxVerifyEnv::new_commit(&header))
    };
    let result = SinceVerifier::new(
        Arc::clone(&rtx),
        Arc::clone(&consensus),
        median_time_context.clone(),
        tx_env,
    )
    .verify();
    assert_error_eq!(result.unwrap_err(), TransactionError::Immature { index: 0 });

    let tx_env = {
        let epoch = EpochNumberWithFraction::new(16, 5, 10);
        let header = HeaderView::new_advanced_builder()
            .number(block_number)
            .epoch(epoch)
            .parent_hash(parent_hash.as_ref().to_owned())
            .build();
        Arc::new(TxVerifyEnv::new_commit(&header))
    };
    let result =
        SinceVerifier::new(rtx, Arc::clone(&consensus), median_time_context, tx_env).verify();
    assert!(result.is_ok());
}

#[test]
fn test_fraction_epoch_since_verify_v2021() {
    let median_time_context = MockMedianTime::new(vec![0; 11]);
    let transaction_info =
        median_time_context.get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1);
    let tx1 = create_tx_with_lock(0x2000_0a00_0f00_000f);
    let rtx1 = create_resolve_tx_with_transaction_info(&tx1, transaction_info.clone());
    let tx2 = create_tx_with_lock(0x2000_0a00_0500_0010);
    let rtx2 = create_resolve_tx_with_transaction_info(&tx2, transaction_info);

    let tx_env = {
        let block_number = 11;
        let epoch = EpochNumberWithFraction::new(16, 5, 10);
        let parent_hash = Arc::new(median_time_context.get_block_hash(block_number - 1));
        let header = HeaderView::new_advanced_builder()
            .number(block_number)
            .epoch(epoch)
            .parent_hash(parent_hash.as_ref().to_owned())
            .build();
        Arc::new(TxVerifyEnv::new_commit(&header))
    };
    {
        // Test CKB v2021
        let hardfork_switch = HardForks::new_mirana();
        let consensus = Arc::new(
            ConsensusBuilder::default()
                .median_time_block_count(MOCK_MEDIAN_TIME_COUNT)
                .hardfork_switch(hardfork_switch)
                .build(),
        );

        let result = SinceVerifier::new(
            rtx1,
            Arc::clone(&consensus),
            median_time_context.clone(),
            Arc::clone(&tx_env),
        )
        .verify();
        assert_error_eq!(
            result.unwrap_err(),
            TransactionError::InvalidSince { index: 0 }
        );

        let result = SinceVerifier::new(rtx2, consensus, median_time_context, tx_env).verify();
        assert!(result.is_ok(), "result = {result:?}");
    }
}

#[test]
pub fn test_absolute_block_number_lock() {
    // absolute lock until block number 0xa
    let tx = create_tx_with_lock(0x0000_0000_0000_000a);
    let median_time_context = MockMedianTime::new(vec![0; 11]);
    let rtx = create_resolve_tx_with_transaction_info(
        &tx,
        median_time_context.get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
    );

    assert_error_eq!(
        verify_since(Arc::clone(&rtx), median_time_context.clone(), 5, 1).unwrap_err(),
        TransactionError::Immature { index: 0 },
    );
    // spent after 10 height
    assert!(verify_since(rtx, median_time_context, 10, 1).is_ok());
}

#[test]
pub fn test_absolute_epoch_number_lock() {
    // absolute lock until epoch number 0xa
    let tx = create_tx_with_lock(0x2000_0100_0000_000a);
    let median_time_context = MockMedianTime::new(vec![0; 11]);
    let rtx = create_resolve_tx_with_transaction_info(
        &tx,
        median_time_context.get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
    );

    assert_error_eq!(
        verify_since(Arc::clone(&rtx), median_time_context.clone(), 5, 1).unwrap_err(),
        TransactionError::Immature { index: 0 },
    );
    // spent after 10 epoch
    assert!(verify_since(rtx, median_time_context, 100, 10).is_ok());
}

#[test]
pub fn test_relative_timestamp_lock() {
    // relative lock timestamp lock
    let tx = create_tx_with_lock(0xc000_0000_0000_0002);
    let median_time_context = MockMedianTime::new(vec![0; 11]);
    let rtx = create_resolve_tx_with_transaction_info(
        &tx,
        median_time_context.get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
    );

    assert_error_eq!(
        verify_since(Arc::clone(&rtx), median_time_context, 4, 1).unwrap_err(),
        TransactionError::Immature { index: 0 },
    );

    // spent after 1024 seconds
    // fake median time: 1124
    let median_time_context =
        MockMedianTime::new(vec![0, 100_000, 1_124_000, 2_000_000, 3_000_000]);
    let rtx = create_resolve_tx_with_transaction_info(
        &tx,
        median_time_context.get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
    );
    assert!(verify_since(rtx, median_time_context, 4, 1).is_ok());
}

#[test]
pub fn test_relative_epoch() {
    // next epoch
    let tx = create_tx_with_lock(0xa000_1000_0000_0002);
    let median_time_context = MockMedianTime::new(vec![0; 11]);
    let rtx = create_resolve_tx_with_transaction_info(
        &tx,
        median_time_context.get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
    );

    assert_error_eq!(
        verify_since(Arc::clone(&rtx), median_time_context.clone(), 4, 1).unwrap_err(),
        TransactionError::Immature { index: 0 },
    );

    assert!(verify_since(rtx, median_time_context, 4, 2).is_ok());
}

#[test]
pub fn test_since_both() {
    // both
    let tx = TransactionBuilder::default()
        .inputs(vec![
            // absolute lock until epoch number 0xa
            CellInput::new(OutPoint::new(h256!("0x1").into(), 0), 0x0000_0000_0000_000a),
            // relative lock until after 2 blocks
            CellInput::new(OutPoint::new(h256!("0x1").into(), 0), 0xc000_0000_0000_0002),
        ])
        .build();
    // spent after 1024 seconds and 4 blocks (less than 10 blocks)
    // fake median time: 1124
    let median_time_context =
        MockMedianTime::new(vec![0, 100_000, 1_124_000, 2_000_000, 3_000_000]);

    let rtx = create_resolve_tx_with_transaction_info(
        &tx,
        median_time_context.get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
    );

    assert_error_eq!(
        verify_since(Arc::clone(&rtx), median_time_context, 4, 1).unwrap_err(),
        TransactionError::Immature { index: 0 },
    );
    // spent after 1024 seconds and 10 blocks
    // fake median time: 1124
    let median_time_context = MockMedianTime::new(vec![
        0, 1, 2, 3, 4, 100_000, 1_124_000, 2_000_000, 3_000_000, 4_000_000, 5_000_000, 6_000_000,
    ]);
    let rtx = create_resolve_tx_with_transaction_info(
        &tx,
        median_time_context.get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
    );

    assert!(verify_since(rtx, median_time_context, 10, 1).is_ok());
}

#[test]
fn test_since_overflow() {
    // use max value for each flag
    for flag in &[
        0b0000_0000u64, // absolute & block
        0b1000_0000u64, // relative & block
        0b0100_0000u64, // absolute & time
        0b1100_0000u64, // relative & time
    ] {
        let tx = create_tx_with_lock((flag << 56) + 0xffff_ffff_ffffu64);
        let median_time_context = MockMedianTime::new(vec![0; 11]);
        let rtx = create_resolve_tx_with_transaction_info(
            &tx,
            median_time_context.get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
        );

        assert_error_eq!(
            verify_since(Arc::clone(&rtx), median_time_context, 5, 1).unwrap_err(),
            TransactionError::Immature { index: 0 },
        );
    }

    for flag in &[
        0b0010_0000u64, // absolute & epoch
        0b1010_0000u64, // relative & epoch
    ] {
        let tx = create_tx_with_lock((flag << 56) + 0xffff_ffff_ffffu64);
        let median_time_context = MockMedianTime::new(vec![0; 11]);
        let rtx = create_resolve_tx_with_transaction_info(
            &tx,
            median_time_context.get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
        );

        assert_error_eq!(
            verify_since(rtx, median_time_context, 5, 1).unwrap_err(),
            TransactionError::InvalidSince { index: 0 },
        );
    }
}

#[test]
fn test_since_timestamp_metric_overflow() {
    for since in [
        0x40ff_ffff_ffff_ffffu64, // absolute timestamp with max 56-bit value
        0xc0ff_ffff_ffff_ffffu64, // relative timestamp with max 56-bit value
    ] {
        let tx = create_tx_with_lock(since);
        let median_time_context = MockMedianTime::new(vec![0; 11]);
        let rtx = create_resolve_tx_with_transaction_info(
            &tx,
            median_time_context.get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
        );

        assert_error_eq!(
            verify_since(rtx, median_time_context, 5, 1).unwrap_err(),
            TransactionError::InvalidSince { index: 0 },
        );
    }
}

#[test]
fn test_relative_since_timestamp_add_overflow() {
    let tx = create_tx_with_lock(0xc000_0000_0000_0000 | (u64::MAX / 1000));
    let median_time_context = MockMedianTime::new(vec![1000; 11]);
    let rtx = create_resolve_tx_with_transaction_info(
        &tx,
        median_time_context.get_transaction_info(1, EpochNumberWithFraction::new(0, 0, 10), 1),
    );

    assert_error_eq!(
        verify_since(rtx, median_time_context, 5, 1).unwrap_err(),
        TransactionError::InvalidSince { index: 0 },
    );
}

#[test]
pub fn test_outputs_data_length_mismatch() {
    let transaction = TransactionBuilder::default()
        .output(CellOutput::default())
        .build();
    let verifier = OutputsDataVerifier::new(&transaction);

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        TransactionError::OutputsDataLengthMismatch {
            outputs_len: 1,
            outputs_data_len: 0
        },
    );

    let transaction = TransactionBuilder::default()
        .output(CellOutput::default())
        .output_data(Bytes::default())
        .build();
    let verifier = OutputsDataVerifier::new(&transaction);

    assert!(verifier.verify().is_ok());
}

fn mock_block_hash(block_number: BlockNumber) -> Byte32 {
    let vec: Vec<u8> = (0..32).map(|_| block_number as u8).collect();
    Byte32::from_slice(vec.as_slice()).unwrap()
}

fn mock_transaction_info(
    block_number: BlockNumber,
    block_epoch: EpochNumberWithFraction,
    index: usize,
) -> TransactionInfo {
    let block_hash = mock_block_hash(block_number);
    TransactionInfo {
        block_number,
        block_epoch,
        block_hash,
        index,
    }
}

// This is a CellDataProvider that always returns None when called.
// As a result, it will only rely on the users to provide cell data
// in CellMeta structure. For our tests on DaoScriptSizeVerifier, it
// perfectly does the job.
struct EmptyDataProvider;

impl CellDataProvider for EmptyDataProvider {
    fn get_cell_data(&self, _out_point: &OutPoint) -> Option<Bytes> {
        None
    }

    fn get_cell_data_hash(&self, _out_point: &OutPoint) -> Option<Byte32> {
        None
    }
}

struct UnexpectedDataProvider;

impl CellDataProvider for UnexpectedDataProvider {
    fn get_cell_data(&self, _out_point: &OutPoint) -> Option<Bytes> {
        panic!("non-DAO verification must not load cell data")
    }

    fn get_cell_data_hash(&self, _out_point: &OutPoint) -> Option<Byte32> {
        panic!("non-DAO verification must not load a cell-data hash")
    }
}

struct ObservedDataProvider<'a> {
    reads: &'a Cell<usize>,
    data: Option<Bytes>,
}

impl CellDataProvider for ObservedDataProvider<'_> {
    fn get_cell_data(&self, _out_point: &OutPoint) -> Option<Bytes> {
        self.reads.set(self.reads.get() + 1);
        self.data.clone()
    }

    fn get_cell_data_hash(&self, _out_point: &OutPoint) -> Option<Byte32> {
        self.reads.set(self.reads.get() + 1);
        self.data.as_deref().map(CellOutput::calc_data_hash)
    }
}

fn build_consensus_with_dao_limiting_block(block_number: u64) -> (Arc<Consensus>, Script) {
    let dao_script = build_genesis_type_id_script(OUTPUT_INDEX_DAO);
    let mut consensus = ConsensusBuilder::default()
        .starting_block_limiting_dao_withdrawing_lock(block_number)
        .build();

    // Default consensus built this way only has one dummy output in the
    // cellbase transaction from genesis block, meaning it will be missing
    // the dao script. For simplicity, we are hacking consensus here with
    // a dao_type_hash value, a proper way should be creating a proper genesis
    // block here, but we will leave it till we really need it.
    consensus.dao_type_hash = dao_script.calc_script_hash();

    let dao_type_script = Script::new_builder()
        .code_hash(dao_script.calc_script_hash())
        .hash_type(ScriptHashType::Type)
        .build();

    (Arc::new(consensus), dao_type_script)
}

fn build_normal_cell_output() -> CellOutput {
    CellOutput::new_builder()
        .capacity(capacity_bytes!(200))
        .build()
}

fn build_dao_cell_output(dao_type_script: &Script) -> CellOutput {
    CellOutput::new_builder()
        .capacity(capacity_bytes!(200))
        .lock(Script::new_builder().args(Bytes::new()).build())
        .type_(Some(dao_type_script.clone()))
        .build()
}

fn build_input_cell_meta(cell_output: CellOutput, data: Bytes) -> CellMeta {
    CellMetaBuilder::from_cell_output(cell_output, data)
        .transaction_info(mock_transaction_info(
            20011,
            EpochNumberWithFraction::new(10, 0, 10),
            0,
        ))
        .build()
}

#[test]
fn dao_data_load_predicate_keeps_the_non_dao_path_provider_free() {
    let (consensus, _) = build_consensus_with_dao_limiting_block(20000);
    let transaction = TransactionBuilder::default()
        .output(build_normal_cell_output())
        .output_data(Bytes::new())
        .build();
    let rtx = Arc::new(ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![build_input_cell_meta(
            build_normal_cell_output(),
            Bytes::new(),
        )],
        resolved_dep_groups: Vec::new(),
    });
    let verifier = DaoScriptSizeVerifier::new(rtx, consensus, UnexpectedDataProvider);

    assert!(!verifier.may_load_cell_data());
    assert!(verifier.verify().is_ok());
}

#[test]
fn dao_data_load_predicate_covers_a_same_index_dao_pair() {
    let (consensus, dao_type_script) = build_consensus_with_dao_limiting_block(20000);
    let transaction = TransactionBuilder::default()
        .output(build_dao_cell_output(&dao_type_script))
        .output_data(Bytes::from(vec![0; 8]))
        .build();
    let rtx = Arc::new(ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![build_input_cell_meta(
            build_dao_cell_output(&dao_type_script),
            Bytes::from(vec![0; 8]),
        )],
        resolved_dep_groups: Vec::new(),
    });
    let verifier = DaoScriptSizeVerifier::new(rtx, consensus, EmptyDataProvider);

    assert!(verifier.may_load_cell_data());
    assert!(verifier.verify().is_ok());
}

#[test]
fn dao_data_load_predicate_covers_observed_provider_reads() {
    let (consensus, dao_type_script) = build_consensus_with_dao_limiting_block(20000);
    // All DAO/non-DAO layouts through two cells, including unequal lengths
    // and DAO cells at different indices. The oracle observes actual I/O.
    let layouts: &[&[bool]] = &[
        &[],
        &[false],
        &[true],
        &[false, false],
        &[false, true],
        &[true, false],
        &[true, true],
    ];
    let output = |dao| {
        if dao {
            build_dao_cell_output(&dao_type_script)
        } else {
            build_normal_cell_output()
        }
    };
    for inputs in layouts {
        for outputs in layouts {
            for data in [
                None,
                Some(Bytes::from(vec![0; 8])),
                Some(Bytes::from(vec![1; 8])),
            ] {
                let transaction = TransactionBuilder::default()
                    .outputs(outputs.iter().map(|&dao| output(dao)).collect::<Vec<_>>())
                    .outputs_data(vec![Bytes::from(vec![1; 8]).pack(); outputs.len()])
                    .build();
                let resolved_inputs = inputs
                    .iter()
                    .map(|&dao| {
                        let mut cell = build_input_cell_meta(output(dao), Bytes::from(vec![0; 8]));
                        cell.mem_cell_data = None;
                        cell.mem_cell_data_hash = None;
                        cell
                    })
                    .collect();
                let rtx = Arc::new(ResolvedTransaction {
                    transaction,
                    resolved_inputs,
                    resolved_cell_deps: Vec::new(),
                    resolved_dep_groups: Vec::new(),
                });
                let reads = Cell::new(0);
                let verifier = DaoScriptSizeVerifier::new(
                    rtx,
                    Arc::clone(&consensus),
                    ObservedDataProvider {
                        reads: &reads,
                        data,
                    },
                );
                let may_load = verifier.may_load_cell_data();
                let _ = verifier.verify();
                // With uncached cells this matrix needs I/O exactly when the
                // predicate says it may. Cached cells need not perform I/O.
                assert_eq!(
                    may_load,
                    reads.get() > 0,
                    "inputs={inputs:?}, outputs={outputs:?}"
                );
            }
        }
    }
}

#[test]
fn test_dao_rejects_withdrawing_mask_alias_output() {
    let (consensus, dao_type_script) = build_consensus_with_dao_limiting_block(20000);
    let mut outputs = vec![build_normal_cell_output(); 33];
    outputs[0] = build_dao_cell_output(&dao_type_script);
    outputs[32] = build_dao_cell_output(&dao_type_script);

    let mut outputs_data = vec![Bytes::new().into(); 33];
    outputs_data[0] = Bytes::from(vec![1; 8]).into();
    outputs_data[32] = Bytes::from(vec![1; 8]).into();

    let transaction = TransactionBuilder::default()
        .outputs(outputs)
        .outputs_data(outputs_data)
        .build();

    let mut resolved_inputs =
        vec![build_input_cell_meta(build_normal_cell_output(), Bytes::new()); 33];
    resolved_inputs[32] = build_input_cell_meta(
        build_dao_cell_output(&dao_type_script),
        Bytes::from(vec![0; 8]),
    );

    let rtx = Arc::new(ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs,
        resolved_dep_groups: vec![],
    });
    let verifier = DaoScriptSizeVerifier::new(rtx, consensus, EmptyDataProvider {});

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        TransactionError::DaoOutputDataMismatch { index: 0 },
    );
}

#[test]
fn test_dao_allows_same_index_withdrawing_output_data() {
    let (consensus, dao_type_script) = build_consensus_with_dao_limiting_block(20000);
    let mut outputs = vec![build_normal_cell_output(); 33];
    outputs[32] = build_dao_cell_output(&dao_type_script);

    let mut outputs_data = vec![Bytes::new().into(); 33];
    outputs_data[32] = Bytes::from(vec![1; 8]).into();

    let transaction = TransactionBuilder::default()
        .outputs(outputs)
        .outputs_data(outputs_data)
        .build();

    let mut resolved_inputs =
        vec![build_input_cell_meta(build_normal_cell_output(), Bytes::new()); 33];
    resolved_inputs[32] = build_input_cell_meta(
        build_dao_cell_output(&dao_type_script),
        Bytes::from(vec![0; 8]),
    );

    let rtx = Arc::new(ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs,
        resolved_dep_groups: vec![],
    });
    let verifier = DaoScriptSizeVerifier::new(rtx, consensus, EmptyDataProvider {});

    assert!(verifier.verify().is_ok());
}

#[test]
fn test_dao_allows_new_deposit_output_data() {
    let (consensus, dao_type_script) = build_consensus_with_dao_limiting_block(20000);
    let mut outputs = vec![build_normal_cell_output(); 2];
    outputs[0] = build_dao_cell_output(&dao_type_script);

    let mut outputs_data = vec![Bytes::new().into(); 2];
    outputs_data[0] = Bytes::from(vec![0; 8]).into();

    let transaction = TransactionBuilder::default()
        .outputs(outputs)
        .outputs_data(outputs_data)
        .build();

    let resolved_inputs = vec![
        build_input_cell_meta(build_normal_cell_output(), Bytes::new()),
        build_input_cell_meta(build_normal_cell_output(), Bytes::new()),
    ];

    let rtx = Arc::new(ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs,
        resolved_dep_groups: vec![],
    });
    let verifier = DaoScriptSizeVerifier::new(rtx, consensus, EmptyDataProvider {});

    assert!(verifier.verify().is_ok());
}

#[test]
fn test_dao_disables_different_lock_script_size() {
    let (consensus, dao_type_script) = build_consensus_with_dao_limiting_block(20000);

    let transaction = TransactionBuilder::default()
        .outputs(vec![
            CellOutput::new_builder()
                .capacity(capacity_bytes!(50))
                .build(),
            CellOutput::new_builder()
                .capacity(capacity_bytes!(200))
                .lock(Script::new_builder().args(Bytes::from(vec![1; 20])).build())
                .type_(Some(dao_type_script.clone()))
                .build(),
        ])
        .outputs_data(vec![Bytes::new().into(); 2])
        .build();

    let rtx = Arc::new(ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(50))
                    .build(),
                Bytes::new(),
            )
            .transaction_info(mock_transaction_info(
                20010,
                EpochNumberWithFraction::new(10, 0, 10),
                0,
            ))
            .build(),
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(201))
                    .lock(Script::new_builder().args(Bytes::new()).build())
                    .type_(Some(dao_type_script))
                    .build(),
                Bytes::from(vec![0; 8]),
            )
            .transaction_info(mock_transaction_info(
                20011,
                EpochNumberWithFraction::new(10, 0, 10),
                0,
            ))
            .build(),
        ],
        resolved_dep_groups: vec![],
    });
    let verifier = DaoScriptSizeVerifier::new(rtx, consensus, EmptyDataProvider {});

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        TransactionError::DaoLockSizeMismatch { index: 1 },
    );
}

#[test]
fn test_dao_disables_different_lock_script_size_before_limiting_block() {
    let (consensus, dao_type_script) = build_consensus_with_dao_limiting_block(21000);

    let transaction = TransactionBuilder::default()
        .outputs(vec![
            CellOutput::new_builder()
                .capacity(capacity_bytes!(50))
                .build(),
            CellOutput::new_builder()
                .capacity(capacity_bytes!(200))
                .lock(Script::new_builder().args(Bytes::from(vec![1; 20])).build())
                .type_(Some(dao_type_script.clone()))
                .build(),
        ])
        .outputs_data(vec![Bytes::new().into(); 2])
        .build();

    let rtx = Arc::new(ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(50))
                    .build(),
                Bytes::new(),
            )
            .transaction_info(mock_transaction_info(
                20010,
                EpochNumberWithFraction::new(10, 0, 10),
                0,
            ))
            .build(),
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(201))
                    .lock(Script::new_builder().args(Bytes::new()).build())
                    .type_(Some(dao_type_script))
                    .build(),
                Bytes::from(vec![0; 8]),
            )
            .transaction_info(mock_transaction_info(
                20011,
                EpochNumberWithFraction::new(10, 0, 10),
                0,
            ))
            .build(),
        ],
        resolved_dep_groups: vec![],
    });
    let verifier = DaoScriptSizeVerifier::new(rtx, consensus, EmptyDataProvider {});

    assert!(verifier.verify().is_ok());
}

#[test]
fn test_non_dao_allows_lock_script_size() {
    let (consensus, _dao_type_script) = build_consensus_with_dao_limiting_block(20000);

    let transaction = TransactionBuilder::default()
        .outputs(vec![
            CellOutput::new_builder()
                .capacity(capacity_bytes!(50))
                .build(),
            CellOutput::new_builder()
                .capacity(capacity_bytes!(200))
                .lock(Script::new_builder().args(Bytes::from(vec![1; 20])).build())
                .build(),
        ])
        .outputs_data(vec![Bytes::new().into(); 2])
        .build();

    let rtx = Arc::new(ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(50))
                    .build(),
                Bytes::new(),
            )
            .transaction_info(mock_transaction_info(
                20010,
                EpochNumberWithFraction::new(10, 0, 10),
                0,
            ))
            .build(),
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(201))
                    .lock(Script::new_builder().args(Bytes::new()).build())
                    .build(),
                Bytes::from(vec![0; 8]),
            )
            .transaction_info(mock_transaction_info(
                20011,
                EpochNumberWithFraction::new(10, 0, 10),
                0,
            ))
            .build(),
        ],
        resolved_dep_groups: vec![],
    });
    let verifier = DaoScriptSizeVerifier::new(rtx, consensus, EmptyDataProvider {});

    assert!(verifier.verify().is_ok());
}

#[test]
fn test_dao_allows_different_lock_script_size_in_withdraw_phase_2() {
    let (consensus, dao_type_script) = build_consensus_with_dao_limiting_block(20000);

    let transaction = TransactionBuilder::default()
        .outputs(vec![
            CellOutput::new_builder()
                .capacity(capacity_bytes!(50))
                .build(),
            CellOutput::new_builder()
                .capacity(capacity_bytes!(200))
                .lock(Script::new_builder().args(Bytes::from(vec![1; 20])).build())
                .type_(Some(dao_type_script.clone()))
                .build(),
        ])
        .outputs_data(vec![Bytes::new().into(); 2])
        .build();

    let rtx = Arc::new(ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(50))
                    .build(),
                Bytes::new(),
            )
            .transaction_info(mock_transaction_info(
                20010,
                EpochNumberWithFraction::new(10, 0, 10),
                0,
            ))
            .build(),
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(201))
                    .lock(Script::new_builder().args(Bytes::new()).build())
                    .type_(Some(dao_type_script))
                    .build(),
                Bytes::from(vec![1; 8]),
            )
            .transaction_info(mock_transaction_info(
                20011,
                EpochNumberWithFraction::new(10, 0, 10),
                0,
            ))
            .build(),
        ],
        resolved_dep_groups: vec![],
    });
    let verifier = DaoScriptSizeVerifier::new(rtx, consensus, EmptyDataProvider {});

    assert!(verifier.verify().is_ok());
}

#[test]
fn test_dao_allows_different_lock_script_size_using_normal_cells_in_withdraw_phase_2() {
    let (consensus, dao_type_script) = build_consensus_with_dao_limiting_block(20000);

    let transaction = TransactionBuilder::default()
        .outputs(vec![
            CellOutput::new_builder()
                .capacity(capacity_bytes!(50))
                .build(),
            CellOutput::new_builder()
                .capacity(capacity_bytes!(200))
                .lock(Script::new_builder().args(Bytes::from(vec![1; 20])).build())
                .type_(Some(dao_type_script))
                .build(),
        ])
        .outputs_data(vec![])
        .build();

    let rtx = Arc::new(ResolvedTransaction {
        transaction,
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(50))
                    .build(),
                Bytes::new(),
            )
            .transaction_info(mock_transaction_info(
                20010,
                EpochNumberWithFraction::new(10, 0, 10),
                0,
            ))
            .build(),
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(201))
                    .lock(Script::new_builder().args(Bytes::new()).build())
                    .build(),
                Bytes::from(vec![1; 8]),
            )
            .transaction_info(mock_transaction_info(
                20011,
                EpochNumberWithFraction::new(10, 0, 10),
                0,
            ))
            .build(),
        ],
        resolved_dep_groups: vec![],
    });
    let verifier = DaoScriptSizeVerifier::new(rtx, consensus, EmptyDataProvider {});

    assert!(verifier.verify().is_ok());
}
