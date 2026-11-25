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
        &RwLock::new(ckb_verification::cache::init_cache()),
        u64::MAX,
        &mut command_rx,
        Some(Duration::from_secs(1)),
    )
    .await
    .unwrap_err();
    assert!(reject.is_verification_interrupted(), "{reject:?}");
    assert!(!reject.is_malformed_tx(), "{reject:?}");
    assert!(reject.is_allowed_relay(), "{reject:?}");
    assert!(!reject.should_recorded(), "{reject:?}");
    assert!(ckb_jsonrpc_types::PoolTransactionReject::try_from(reject).is_err());
}

#[tokio::test]
async fn runner_external_errors_preserve_interruption_and_script_attribution() {
    use ckb_script::{
        SchedulerRunner, ScriptError, VM_INTERRUPTED_MESSAGE, types::TerminatedResult,
    };

    struct ExternalError(&'static str);

    impl<S: Send> SchedulerRunner<S> for ExternalError {
        async fn run(
            &mut self,
            _scheduler: S,
            _max_cycles: Cycle,
        ) -> Result<Option<TerminatedResult>, ckb_vm::Error> {
            Err(ckb_vm::Error::External(self.0.into()))
        }
    }

    let snapshot = snapshot(Arc::new(ConsensusBuilder::default().build()));
    let env = Arc::new(TxVerifyEnv::new_submit(snapshot.tip_header()));
    let verifier = TransactionScriptsVerifier::new(
        program_transaction(
            ScriptVersion::V0,
            &[include_bytes!("../../../script/testdata/always_success")],
        ),
        snapshot.as_data_loader(),
        snapshot.cloned_consensus(),
        env,
    );

    for (message, interrupted) in [
        (VM_INTERRUPTED_MESSAGE, true),
        // Preserve the existing payload independently of the shared constant.
        ("stopped", true),
        ("ordinary VM failure", false),
        ("stopped ", false),
    ] {
        let error = verifier
            .verify_with_runner(u64::MAX, &mut ExternalError(message))
            .await
            .unwrap_err();
        let expected: ckb_error::Error = if interrupted {
            ScriptError::Interrupts
        } else {
            ScriptError::VMInternalError(ckb_vm::Error::External(message.into()))
        }
        .input_lock_script(0)
        .into();
        assert_eq!(error.kind(), expected.kind(), "{message:?}");
        ckb_error::assert_error_eq!(error, expected, "{message:?}");
    }
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
async fn local_and_budgeted_verification_preserve_vm_versions_cycles_and_errors() {
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
                Arc::clone(&rtx),
                snapshot.as_data_loader(),
                snapshot.cloned_consensus(),
                Arc::clone(&env),
            );
            let expected = verifier.verify(u64::MAX);
            let local = verify_rtx(
                Arc::clone(&snapshot),
                Arc::clone(&rtx),
                Arc::clone(&env),
                &RwLock::new(ckb_verification::cache::init_cache()),
                u64::MAX,
                &mut commands,
                None,
            )
            .await;
            let mut runner = execution::VmRunner {
                command: &mut commands,
                remaining: Duration::from_secs(2),
                compute: None,
            };
            let actual = verifier.verify_with_runner(u64::MAX, &mut runner).await;
            match expected {
                Ok(cycles) => {
                    let (outcome, _) = local.unwrap();
                    assert_eq!(outcome.cycles(), cycles);
                    assert_eq!(actual.unwrap(), Some(cycles));
                    ckb_error::assert_error_eq!(
                        verifier
                            .verify_with_runner(cycles - 1, &mut runner)
                            .await
                            .unwrap_err(),
                        verifier.verify(cycles - 1).unwrap_err(),
                    );
                }
                Err(error) => {
                    let Reject::Verification(local) = local.unwrap_err() else {
                        panic!("local verification lost the canonical script error");
                    };
                    assert_eq!(local.to_string(), error.to_string());
                    ckb_error::assert_error_eq!(actual.unwrap_err(), error);
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
        compute: None,
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
        compute: None,
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

#[test]
fn script_group_count_excludes_type_id_and_deduplicates_groups() {
    let lock_a = Script::new_builder()
        .code_hash(H256([0x11; 32]))
        .args(Bytes::from_static(b"a").pack())
        .build();
    let lock_b = Script::new_builder()
        .code_hash(H256([0x11; 32]))
        .args(Bytes::from_static(b"b").pack())
        .build();
    let type_custom = Script::new_builder()
        .code_hash(H256([0x22; 32]))
        .args(Bytes::from_static(b"t").pack())
        .build();
    let type_id = Script::new_builder()
        .code_hash(ckb_chain_spec::consensus::TYPE_ID_CODE_HASH)
        .hash_type(ScriptHashType::Type)
        .args(Bytes::from_static(&[1; 32]).pack())
        .build();
    let tx = TransactionBuilder::default()
        .output(
            CellOutput::new_builder()
                .capacity(capacity_bytes!(100))
                .lock(lock_a.clone())
                .type_(Some(type_custom.clone()).pack())
                .build(),
        )
        .output(
            CellOutput::new_builder()
                .capacity(capacity_bytes!(100))
                .lock(lock_b.clone())
                .type_(Some(type_id.clone()).pack())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .output_data(Bytes::new().pack())
        .build();
    let rtx = ResolvedTransaction {
        transaction: tx,
        resolved_cell_deps: vec![],
        resolved_dep_groups: vec![],
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(100))
                    .lock(lock_a.clone())
                    .type_(Some(type_custom).pack())
                    .build(),
                Bytes::new(),
            )
            .build(),
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(100))
                    .lock(lock_a)
                    .type_(Some(type_id).pack())
                    .build(),
                Bytes::new(),
            )
            .build(),
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(100))
                    .lock(lock_b)
                    .build(),
                Bytes::new(),
            )
            .build(),
        ],
    };
    // 2 unique locks (lock_a deduplicated across 2 inputs, lock_b) + 1 custom type
    // (deduplicated across input/output); Type ID is excluded.
    assert_eq!(script_group_count(&rtx), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn single_worker_script_preparation_keeps_tokio_control_task_runnable() {
    use ckb_types::core::{EpochExt, HeaderView};
    use ckb_verification::{
        CellDataProvider, EpochProvider, ExtensionProvider, HeaderFields, HeaderFieldsProvider,
        HeaderProvider,
    };
    use std::sync::{Mutex as StdMutex, mpsc as std_mpsc};

    #[derive(Clone)]
    struct BlockingPrepLoader<DL> {
        inner: DL,
        target_dep: OutPoint,
        target_data_hash: Byte32,
        entered: std_mpsc::Sender<()>,
        release: Arc<StdMutex<std_mpsc::Receiver<()>>>,
    }

    impl<DL: CellDataProvider> CellDataProvider for BlockingPrepLoader<DL> {
        fn get_cell_data(&self, out_point: &OutPoint) -> Option<Bytes> {
            self.inner.get_cell_data(out_point)
        }

        fn get_cell_data_hash(&self, out_point: &OutPoint) -> Option<Byte32> {
            if *out_point == self.target_dep {
                self.entered.send(()).unwrap();
                self.release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .expect("test must release script preparation barrier");
                return Some(self.target_data_hash.clone());
            }
            self.inner.get_cell_data_hash(out_point)
        }
    }

    impl<DL: HeaderProvider> HeaderProvider for BlockingPrepLoader<DL> {
        fn get_header(&self, hash: &Byte32) -> Option<HeaderView> {
            self.inner.get_header(hash)
        }
    }

    impl<DL: ExtensionProvider> ExtensionProvider for BlockingPrepLoader<DL> {
        fn get_block_extension(&self, hash: &Byte32) -> Option<ckb_types::packed::Bytes> {
            self.inner.get_block_extension(hash)
        }
    }

    impl<DL: HeaderFieldsProvider> HeaderFieldsProvider for BlockingPrepLoader<DL> {
        fn get_header_fields(&self, hash: &Byte32) -> Option<HeaderFields> {
            self.inner.get_header_fields(hash)
        }
    }

    impl<DL: EpochProvider> EpochProvider for BlockingPrepLoader<DL> {
        fn get_epoch_ext(&self, block_header: &HeaderView) -> Option<EpochExt> {
            self.inner.get_epoch_ext(block_header)
        }

        fn get_block_hash(&self, number: ckb_types::core::BlockNumber) -> Option<Byte32> {
            self.inner.get_block_hash(number)
        }

        fn get_block_ext(&self, block_hash: &Byte32) -> Option<ckb_types::core::BlockExt> {
            self.inner.get_block_ext(block_hash)
        }

        fn get_block_header(&self, hash: &Byte32) -> Option<HeaderView> {
            self.inner.get_block_header(hash)
        }
    }

    let snapshot = snapshot(Arc::new(
        ConsensusBuilder::default()
            .hardfork_switch(HardForks::new_dev())
            .build(),
    ));
    let mut rtx = (*program_transaction(
        ScriptVersion::V0,
        &[include_bytes!("../../../script/testdata/always_success")],
    ))
    .clone();
    // Clear precomputed dep hash so `TxData::new` goes through
    // `DataLoader::load_cell_data_hash` -> `CellDataProvider::get_cell_data_hash`.
    let target_data_hash = rtx.resolved_cell_deps[0]
        .mem_cell_data_hash
        .take()
        .expect("fixture cell dep carries data hash");
    let target_dep = rtx.resolved_cell_deps[0].out_point.clone();
    let rtx = Arc::new(rtx);

    let (entered_tx, entered_rx) = std_mpsc::channel();
    let (release_tx, release_rx) = std_mpsc::channel();
    let loader = BlockingPrepLoader {
        inner: snapshot.as_data_loader(),
        target_dep,
        target_data_hash,
        entered: entered_tx,
        release: Arc::new(StdMutex::new(release_rx)),
    };
    let consensus = snapshot.cloned_consensus();
    let env = Arc::new(TxVerifyEnv::new_submit(snapshot.tip_header()));
    let cache = Arc::new(RwLock::new(ckb_verification::cache::init_cache()));
    let compute = Arc::new(ComputePool::new(1));

    let verify_task = {
        let consensus = Arc::clone(&consensus);
        let rtx = Arc::clone(&rtx);
        let env = Arc::clone(&env);
        let cache = Arc::clone(&cache);
        let compute = Arc::clone(&compute);
        tokio::spawn(async move {
            let (_tx, mut commands) = watch::channel(ChunkCommand::Resume);
            verify_rtx_with_loader(
                consensus,
                loader,
                rtx,
                env,
                &cache,
                u64::MAX,
                &mut commands,
                Some(Duration::from_secs(2)),
                Some(&compute),
                #[cfg(any(test, feature = "internal"))]
                None,
            )
            .await
        })
    };

    // Wait on a blocking thread so the sole Tokio worker stays free to drive `verify_task`.
    tokio::task::spawn_blocking(move || {
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("script preparation must enter DataLoader::load_cell_data_hash");
    })
    .await
    .unwrap();

    // While script preparation is paused on ComputePool, an independent Tokio
    // control task on the sole runtime worker must still run and complete.
    let control_done = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::spawn(async {
            tokio::task::yield_now().await;
            42usize
        }),
    )
    .await
    .expect("independent Tokio control task must not starve while script preparation waits")
    .unwrap();
    assert_eq!(control_done, 42);
    assert!(
        !verify_task.is_finished(),
        "verification must still be paused inside script preparation"
    );

    release_tx.send(()).unwrap();
    let (outcome, _timestamp, _spent) = tokio::time::timeout(Duration::from_secs(5), verify_task)
        .await
        .expect("verification completes promptly after preparation resumes")
        .unwrap()
        .expect("valid always_success transaction must verify");
    assert!(outcome.cycles() > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepared_script_verification_binds_cache_key_and_max_cycles_across_cached_and_vm_paths() {
    let snapshot = snapshot(Arc::new(
        ConsensusBuilder::default()
            .hardfork_switch(HardForks::new_dev())
            .build(),
    ));
    let env = Arc::new(TxVerifyEnv::new_submit(snapshot.tip_header()));
    let rtx = program_transaction(
        ScriptVersion::V0,
        &[include_bytes!("../../../script/testdata/always_success")],
    );
    let verifier = ContextualTransactionVerifier::new(
        Arc::clone(&rtx),
        snapshot.cloned_consensus(),
        snapshot.as_data_loader(),
        Arc::clone(&env),
    );
    let exact_cycles = verifier.verify_scripts(u64::MAX, None).unwrap().cycles();
    assert!(exact_cycles > 0);

    // 1. VM execution path: `prepare_for_runner` binds `cache_key` and
    // `max_cycles`; `verify_with_runner` accepts only the runner.
    let (_resume_tx, mut resume_rx) = watch::channel(ChunkCommand::Resume);
    let mut runner = execution::VmRunner {
        command: &mut resume_rx,
        remaining: Duration::from_secs(2),
        compute: None,
    };
    let prepared_exact = verifier.prepare_for_runner(exact_cycles, None).unwrap();
    let executed = prepared_exact
        .verify_with_runner(&mut runner)
        .await
        .unwrap()
        .expect("exact cycle cap must succeed");
    assert!(!executed.was_reused());
    let proof = executed
        .executed_proof()
        .expect("uncached run produces proof");
    assert_eq!(proof.key(), *verifier.cache_key());
    assert_eq!(proof.cycles(), exact_cycles);

    // Tightening `max_cycles` by 1 at preparation time rejects VM execution
    // under the exact bound cap without passing `max_cycles` again.
    let prepared_tight = verifier.prepare_for_runner(exact_cycles - 1, None).unwrap();
    let vm_err = prepared_tight
        .verify_with_runner(&mut runner)
        .await
        .unwrap_err();
    ckb_error::assert_error_eq!(
        vm_err,
        ckb_script::ScriptError::ExceededMaximumCycles(exact_cycles - 1).input_lock_script(0),
    );

    // 2. Cache-hit path: enforces the exact same `max_cycles` cap at preparation
    // time and returns the bound proof without running the VM.
    let (_stop_tx, mut stop_rx) = watch::channel(ChunkCommand::Stop);
    let mut stopped_runner = execution::VmRunner {
        command: &mut stop_rx,
        remaining: Duration::ZERO,
        compute: None,
    };
    let prepared_cached = verifier
        .prepare_for_runner(exact_cycles, Some(proof))
        .unwrap();
    let reused = prepared_cached
        .verify_with_runner(&mut stopped_runner)
        .await
        .unwrap()
        .expect("cached proof within cap must reuse without invoking VM");
    assert!(reused.was_reused());
    assert_eq!(reused.proof(), proof);

    let cached_err = verifier
        .prepare_for_runner(exact_cycles - 1, Some(proof))
        .err()
        .expect("cached proof exceeding bound max_cycles must be rejected");
    ckb_error::assert_error_eq!(
        cached_err,
        ckb_script::ScriptError::ExceededMaximumCycles(exact_cycles - 1).unknown_source(),
    );

    // 3. A proof from another transaction (mismatched cache key) is ignored and
    // falls through to VM execution under the same bound `max_cycles`.
    let mut other_rtx = (*rtx).clone();
    other_rtx.transaction = other_rtx
        .transaction
        .as_advanced_builder()
        .witness(Bytes::from_static(b"different-witness").pack())
        .build();
    let other_verifier = ContextualTransactionVerifier::new(
        Arc::new(other_rtx),
        snapshot.cloned_consensus(),
        snapshot.as_data_loader(),
        env,
    );
    let other_proof = other_verifier
        .verify_scripts(u64::MAX, None)
        .unwrap()
        .executed_proof()
        .unwrap();
    assert_ne!(other_proof.key(), *verifier.cache_key());
    let prepared_mismatch = verifier
        .prepare_for_runner(exact_cycles - 1, Some(other_proof))
        .unwrap();
    let mismatch_err = prepared_mismatch
        .verify_with_runner(&mut runner)
        .await
        .unwrap_err();
    ckb_error::assert_error_eq!(
        mismatch_err,
        ckb_script::ScriptError::ExceededMaximumCycles(exact_cycles - 1).input_lock_script(0),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cached_hit_under_positive_budget_records_one_zero_ratio_sample_on_both_backends() {
    let _ = ckb_metrics::METRICS_SERVICE_ENABLED.set(true);
    let metrics = ckb_metrics::handle().expect("metrics enabled in isolated nextest process");
    let histogram = &metrics.ckb_tx_pool_verify_cpu_budget_ratio;

    let snapshot = snapshot(Arc::new(
        ConsensusBuilder::default()
            .hardfork_switch(HardForks::new_dev())
            .build(),
    ));
    let rtx = program_transaction(
        ScriptVersion::V0,
        &[include_bytes!("../../../script/testdata/always_success")],
    );
    let env = Arc::new(TxVerifyEnv::new_submit(snapshot.tip_header()));
    let verifier = ContextualTransactionVerifier::new(
        Arc::clone(&rtx),
        snapshot.cloned_consensus(),
        snapshot.as_data_loader(),
        Arc::clone(&env),
    );
    let proof = verifier
        .verify_scripts(u64::MAX, None)
        .unwrap()
        .executed_proof()
        .unwrap();
    let cache = RwLock::new(ckb_verification::cache::init_cache());
    cache.read().await.insert_concurrent(proof);

    let compute = ComputePool::new(1);
    let budget = Duration::from_secs(1);
    let (_tx, mut commands) = watch::channel(ChunkCommand::Resume);

    // 1. Production ComputePool backend records exactly one zero-value ratio sample.
    let before_count = histogram.get_sample_count();
    let before_sum = histogram.get_sample_sum();
    let (prod_outcome, _ts, prod_spent) = verify_rtx_with_compute(
        Arc::clone(&snapshot),
        Arc::clone(&rtx),
        Arc::clone(&env),
        &cache,
        u64::MAX,
        &mut commands,
        Some(budget),
        Some(&compute),
        #[cfg(any(test, feature = "internal"))]
        None,
    )
    .await
    .expect("cached production path must succeed");
    assert!(prod_outcome.was_reused());
    assert_eq!(prod_spent, Duration::ZERO);
    assert_eq!(histogram.get_sample_count(), before_count + 1);
    assert_eq!(histogram.get_sample_sum(), before_sum);

    // 2. Fallback backend records exactly one zero-value ratio sample.
    let mid_count = histogram.get_sample_count();
    let mid_sum = histogram.get_sample_sum();
    let (fallback_outcome, _ts, fallback_spent) = verify_rtx_with_compute(
        snapshot,
        rtx,
        env,
        &cache,
        u64::MAX,
        &mut commands,
        Some(budget),
        None,
        #[cfg(any(test, feature = "internal"))]
        None,
    )
    .await
    .expect("cached fallback path must succeed");
    assert!(fallback_outcome.was_reused());
    assert_eq!(fallback_spent, Duration::ZERO);
    assert_eq!(histogram.get_sample_count(), mid_count + 1);
    assert_eq!(histogram.get_sample_sum(), mid_sum);
}
