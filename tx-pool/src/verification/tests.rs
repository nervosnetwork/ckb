use super::*;
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_db::RocksDB;
use ckb_script::{ScriptVersion, TransactionScriptsVerifier};
use ckb_store::ChainDB;
use ckb_types::{
    H256, U256,
    bytes::Bytes,
    core::{
        Capacity, ScriptHashType, TransactionBuilder, capacity_bytes,
        cell::{CellMetaBuilder, ResolvedTransaction},
        hardfork::HardForks,
    },
    packed::{CellDep, CellInput, CellOutput, OutPoint, Script},
    prelude::*,
};
use std::time::Duration;
use tempfile::TempDir;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopped_verification_is_retryable_without_a_persistent_rejection() {
    let rtx = program_transaction(
        ScriptVersion::V0,
        &[include_bytes!("../../../script/testdata/always_success")],
    );
    let snapshot = snapshot(Arc::new(ConsensusBuilder::default().build()));
    let env = Arc::new(TxVerifyEnv::new_submit(snapshot.tip_header()));
    let (_commands, mut command_rx) = watch::channel(ChunkCommand::Stop);
    let reject = verify_rtx(
        snapshot,
        rtx,
        env,
        None,
        u64::MAX,
        &mut command_rx,
        Some(TxPoolVerificationBudget::new(
            Duration::from_secs(1),
            ComputeMode::YieldRuntimeWorker,
        )),
    )
    .await
    .unwrap_err();
    assert!(reject.is_verification_interrupted(), "{reject:?}");
    assert!(!reject.is_malformed_tx(), "{reject:?}");
    assert!(reject.is_allowed_relay(), "{reject:?}");
    assert!(!reject.should_recorded(), "{reject:?}");
    assert!(ckb_jsonrpc_types::PoolTransactionReject::try_from(reject).is_err());
}

pub(super) fn snapshot(consensus: Arc<ckb_chain_spec::consensus::Consensus>) -> Arc<Snapshot> {
    let tmp = TempDir::new().unwrap();
    let store = ChainDB::new(RocksDB::open_in(&tmp, 1), Default::default());
    Arc::new(Snapshot::new(
        consensus.genesis_block().header(),
        U256::zero(),
        Default::default(),
        store.get_snapshot(),
        Default::default(),
        consensus,
    ))
}

pub(super) fn program_transaction(
    version: ScriptVersion,
    programs: &[&'static [u8]],
) -> Arc<ResolvedTransaction> {
    let lock = Script::new_builder()
        .code_hash(CellOutput::calc_data_hash(&Bytes::from_static(programs[0])))
        .hash_type(version.data_hash_type())
        .build();
    transaction(lock, programs)
}

fn transaction(lock: Script, programs: &[&'static [u8]]) -> Arc<ResolvedTransaction> {
    let input = OutPoint::new(H256([1; 32]).into(), 0);
    let cells: Vec<_> = programs
        .iter()
        .enumerate()
        .map(|(index, program)| {
            CellMetaBuilder::from_cell_output(CellOutput::default(), Bytes::from_static(program))
                .out_point(OutPoint::new(H256([2; 32]).into(), index as u32))
                .build()
        })
        .collect();
    let tx = TransactionBuilder::default()
        .input(CellInput::new(input.clone(), 0))
        .cell_deps(cells.iter().map(|cell| {
            CellDep::new_builder()
                .out_point(cell.out_point.clone())
                .build()
        }))
        .output(
            CellOutput::new_builder()
                .capacity(capacity_bytes!(100))
                .lock(lock.clone())
                .build(),
        )
        .output_data(Bytes::new())
        .build();
    Arc::new(ResolvedTransaction {
        transaction: tx,
        resolved_cell_deps: cells,
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(200))
                    .lock(lock)
                    .build(),
                Bytes::new(),
            )
            .out_point(input)
            .build(),
        ],
        resolved_dep_groups: vec![],
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn budgeted_verification_preserves_vm_versions_cycles_and_errors() {
    let snapshot = snapshot(Arc::new(
        ConsensusBuilder::default()
            .hardfork_switch(HardForks::new_dev())
            .build(),
    ));
    let env = Arc::new(TxVerifyEnv::new_submit(snapshot.tip_header()));
    let (_commands, mut commands) = watch::channel(ChunkCommand::Resume);
    for version in [ScriptVersion::V0, ScriptVersion::V1, ScriptVersion::V2] {
        for program in [
            include_bytes!("../../../script/testdata/always_success").as_slice(),
            include_bytes!("../../../script/testdata/always_failure").as_slice(),
        ] {
            let rtx = program_transaction(version, &[program]);
            let verifier = TransactionScriptsVerifier::new(
                rtx,
                snapshot.as_data_loader(),
                snapshot.cloned_consensus(),
                Arc::clone(&env),
            );
            for mode in [ComputeMode::Inline, ComputeMode::YieldRuntimeWorker] {
                let expected = verifier.verify(u64::MAX);
                let mut runner = execution::VmRunner {
                    command: &mut commands,
                    remaining: Duration::from_secs(2),
                    mode,
                };
                let actual = verifier.verify_with_runner(u64::MAX, &mut runner).await;
                match expected {
                    Ok(cycles) => {
                        assert_eq!(actual.unwrap(), Some(cycles));
                        ckb_error::assert_error_eq!(
                            verifier
                                .verify_with_runner(cycles - 1, &mut runner)
                                .await
                                .unwrap_err(),
                            verifier.verify(cycles - 1).unwrap_err(),
                        );
                    }
                    Err(error) => ckb_error::assert_error_eq!(actual.unwrap_err(), error),
                }
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn real_spawned_vm_cannot_outlive_its_budget() {
    let snapshot = snapshot(Arc::new(
        ConsensusBuilder::default()
            .hardfork_switch(HardForks::new_dev())
            .build(),
    ));
    let rtx = program_transaction(
        ScriptVersion::V2,
        &[
            include_bytes!("../../../script/testdata/spawn_caller_exec"),
            include_bytes!("../../../script/testdata/infinite_loop"),
        ],
    );
    let env = Arc::new(TxVerifyEnv::new_submit(snapshot.tip_header()));
    let verifier = TransactionScriptsVerifier::new(
        rtx,
        snapshot.as_data_loader(),
        snapshot.cloned_consensus(),
        env,
    );
    let (_commands, mut commands) = watch::channel(ChunkCommand::Resume);
    let mut runner = execution::VmRunner {
        command: &mut commands,
        remaining: Duration::from_millis(20),
        mode: ComputeMode::YieldRuntimeWorker,
    };
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        verifier.verify_with_runner(u64::MAX, &mut runner),
    )
    .await
    .unwrap();
    assert!(result.unwrap().is_none());
    assert_eq!(runner.remaining, Duration::ZERO);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn type_id_and_cached_proofs_skip_vm_budget_without_skipping_validation() {
    let snapshot = snapshot(Arc::new(ConsensusBuilder::default().build()));
    let type_id = Script::new_builder()
        .code_hash(ckb_chain_spec::consensus::TYPE_ID_CODE_HASH)
        .hash_type(ScriptHashType::Type)
        .args(Bytes::from_static(&[1; 32]))
        .build();
    let env = Arc::new(TxVerifyEnv::new_submit(snapshot.tip_header()));
    let verifier = |rtx| {
        ContextualTransactionVerifier::new(
            rtx,
            snapshot.cloned_consensus(),
            snapshot.as_data_loader(),
            Arc::clone(&env),
        )
    };
    let (_commands, mut commands) = watch::channel(ChunkCommand::Stop);
    let mut runner = execution::VmRunner {
        command: &mut commands,
        remaining: Duration::ZERO,
        mode: ComputeMode::YieldRuntimeWorker,
    };
    let type_id = verifier(transaction(type_id, &[]));
    assert_eq!(
        type_id
            .verify_with_runner(u64::MAX, None, &mut runner)
            .await
            .unwrap()
            .unwrap()
            .cycles(),
        type_id.verify_scripts(u64::MAX, None).unwrap().cycles()
    );

    // An ordinary script needs a valid proof to bypass the exhausted VM budget.
    let rtx = program_transaction(
        ScriptVersion::V0,
        &[include_bytes!("../../../script/testdata/always_success")],
    );
    let ordinary = verifier(Arc::clone(&rtx));
    assert!(
        ordinary
            .verify_with_runner(u64::MAX, None, &mut runner)
            .await
            .unwrap()
            .is_none()
    );
    let proof = ordinary
        .verify_scripts(u64::MAX, None)
        .unwrap()
        .executed_proof()
        .unwrap();
    let reused = ordinary
        .verify_with_runner(proof.cycles(), Some(proof), &mut runner)
        .await
        .unwrap()
        .unwrap();
    assert!(reused.was_reused());
    assert_eq!(reused.cycles(), proof.cycles());
    ckb_error::assert_error_eq!(
        ordinary
            .verify_with_runner(proof.cycles() - 1, Some(proof), &mut runner)
            .await
            .unwrap_err(),
        ckb_script::ScriptError::ExceededMaximumCycles(proof.cycles() - 1).unknown_source(),
    );

    let mut invalid = (*rtx).clone();
    invalid.resolved_inputs[0].cell_output = invalid.resolved_inputs[0]
        .cell_output
        .clone()
        .as_builder()
        .capacity(capacity_bytes!(99))
        .build();
    ckb_error::assert_error_eq!(
        verifier(Arc::new(invalid.clone()))
            .verify_with_runner(0, Some(proof), &mut runner)
            .await
            .unwrap_err(),
        ckb_verification::TransactionError::OutputsSumOverflow {
            inputs_sum: capacity_bytes!(99),
            outputs_sum: capacity_bytes!(100)
        },
    );
    invalid.transaction = invalid
        .transaction
        .as_advanced_builder()
        .set_inputs(vec![
            invalid
                .transaction
                .inputs()
                .get(0)
                .unwrap()
                .as_builder()
                .since(1_000u64)
                .build(),
        ])
        .build();
    ckb_error::assert_error_eq!(
        verifier(Arc::new(invalid))
            .verify_with_runner(0, Some(proof), &mut runner)
            .await
            .unwrap_err(),
        ckb_verification::TransactionError::Immature { index: 0 },
    );
}
