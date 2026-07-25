//! End-to-end tests for the tx-pool resolve -> verify -> submit pipeline.

use crate::component::pool_map::Status;
use crate::process::PreCheckedTx;
use crate::service::TxPoolService;
use crate::tx_source::TxSource;
use ckb_app_config::{TxPoolConfig, VerifyOrdering};
use ckb_chain_spec::consensus::Consensus;
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_crypto::secp::Privkey;
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_stop_handler::CancellationToken;
use ckb_store::attach_block_cell;
use ckb_system_scripts::BUNDLED_CELL;
use ckb_test_chain_utils::{MockStore, always_success_cell};
use ckb_types::{
    H160, H256, U256,
    bytes::Bytes,
    core::{
        BlockBuilder, BlockExt, BlockView, Capacity, EpochNumberWithFraction, TransactionBuilder,
        TransactionView,
    },
    h160, h256,
    packed::{CellDep, CellInput, CellOutput, OutPoint, ProposalShortId, Script, WitnessArgs},
    prelude::*,
    utilities::difficulty_to_compact,
    utilities::merkle_mountain_range::ChainRootMMR,
};
use ckb_verification::{
    TxVerifyEnv,
    cache::{Completed, TxVerificationCacheKey},
};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

const MAX_TX_VERIFY_CYCLES: u64 = 70_000_000;
const ISSUE_OUTPUT_CAPACITY: u64 = 5_000;

#[test]
fn unusable_pipeline_residency_budget_fails_at_startup() {
    let (consensus, _) = test_consensus(1);
    let mut config = tx_pool_config();
    config.max_tx_pipeline_resident_size = 0;
    let shutdown = CancellationToken::new();
    let result = catch_unwind(AssertUnwindSafe(|| {
        crate::component::pipeline_runtime::PipelineRuntime::new(&config, &consensus, shutdown)
    }));
    assert!(
        result.is_err(),
        "zero must not be silently promoted to an unusable one-byte budget"
    );
}

#[test]
fn pipeline_runtime_panics_fail_closed_instead_of_recovering_poisoned_state() {
    let (consensus, _) = test_consensus(1);
    let shutdown = CancellationToken::new();
    let runtime = crate::component::pipeline_runtime::PipelineRuntime::new(
        &tx_pool_config(),
        &consensus,
        shutdown.clone(),
    );
    let tx = TransactionBuilder::default().build();

    let injected = catch_unwind(AssertUnwindSafe(|| {
        let _ = runtime.admit_transaction_journaled(
            tx,
            TxSource::Local,
            0,
            crate::component::pipeline_coordinator::RawStage::PreCheck,
            |_| panic!("injected journal failure after coordinator admission"),
        );
    }));
    assert!(
        injected.is_err(),
        "the injected panic must escape the boundary"
    );
    assert!(runtime.is_failed(), "the runtime must latch failure");
    assert!(
        runtime.pool_persistence_safe(),
        "a coordinator-only panic must not discard a coherent accepted pool"
    );
    assert!(
        shutdown.is_cancelled(),
        "a fatal coordinator failure must stop the tx-pool service generation"
    );

    let reused = catch_unwind(AssertUnwindSafe(|| runtime.read(|_| ())));
    assert!(
        reused.is_err(),
        "poisoned coordinator state must never be recovered into service"
    );
}

#[test]
fn authoritative_boundary_failure_disables_pool_persistence() {
    let (consensus, _) = test_consensus(1);
    let shutdown = CancellationToken::new();
    let runtime = crate::component::pipeline_runtime::PipelineRuntime::new(
        &tx_pool_config(),
        &consensus,
        shutdown.clone(),
    );

    let injected = catch_unwind(AssertUnwindSafe(|| {
        runtime.guard_authoritative_mutation("injected pool boundary", || {
            panic!("injected partial PoolMap mutation")
        });
    }));
    assert!(injected.is_err());
    assert!(runtime.is_failed());
    assert!(shutdown.is_cancelled());
    assert!(
        !runtime.pool_persistence_safe(),
        "an interrupted authoritative pool mutation is not a recovery point"
    );
}

#[test]
fn inconsistent_ingress_source_attribution_is_fail_closed() {
    use crate::component::pipeline_coordinator::CoordinatorSource;
    use crate::component::pipeline_runtime::PipelineRawTx;

    let (consensus, _) = test_consensus(1);
    let shutdown = CancellationToken::new();
    let runtime = crate::component::pipeline_runtime::PipelineRuntime::new(
        &tx_pool_config(),
        &consensus,
        shutdown.clone(),
    );
    let raw = PipelineRawTx::new(TransactionBuilder::default().build(), TxSource::Local, 0);

    let mismatch = catch_unwind(AssertUnwindSafe(|| {
        runtime.require_authoritative_source(
            &raw,
            CoordinatorSource::Remote(ckb_network::PeerIndex::from(7)),
        );
    }));
    assert!(mismatch.is_err());
    assert!(runtime.is_failed());
    assert!(shutdown.is_cancelled());
}

#[test]
fn coordinator_invariant_error_cannot_be_downgraded_to_transaction_reject() {
    use crate::component::pipeline_coordinator::{CoordinatorError, QueueKind};

    let (consensus, _) = test_consensus(1);
    let shutdown = CancellationToken::new();
    let runtime = crate::component::pipeline_runtime::PipelineRuntime::new(
        &tx_pool_config(),
        &consensus,
        shutdown.clone(),
    );
    let failed = catch_unwind(AssertUnwindSafe(|| {
        runtime.reject_or_fail(
            "injected production adapter invariant",
            CoordinatorError::QueueInvariant(QueueKind::Resolve),
        );
    }));
    assert!(failed.is_err());
    assert!(runtime.is_failed());
    assert!(shutdown.is_cancelled());
}

#[test]
fn weaker_duplicate_source_cannot_amplify_into_pipeline_fail_stop() {
    use crate::component::pipeline_coordinator::{CoordinatorSource, RawStage};

    let (consensus, _) = test_consensus(1);
    let shutdown = CancellationToken::new();
    let runtime = crate::component::pipeline_runtime::PipelineRuntime::new(
        &tx_pool_config(),
        &consensus,
        shutdown.clone(),
    );
    let proposal = TransactionBuilder::default()
        .witness(Bytes::from_static(b"proposal"))
        .build();
    let hash = proposal.hash();
    let proposal_witness = proposal.witness_hash();
    let local_variant = proposal
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"local-history").pack()])
        .build();
    assert_eq!(local_variant.hash(), hash);
    assert_ne!(local_variant.witness_hash(), proposal_witness);

    assert!(
        runtime
            .admit_transaction(proposal, TxSource::Proposal, 0, RawStage::PreCheck)
            .expect("proposal admission")
            .0
    );
    let (added, terminal) = runtime
        .admit_transaction(local_variant, TxSource::Local, 0, RawStage::Resolve)
        .expect("weaker duplicate is an ownership no-op");
    assert!(!added);
    assert!(terminal.is_empty());
    runtime.read(|coordinator| {
        let view = coordinator.view(&hash).expect("proposal owner remains");
        assert_eq!(view.source, CoordinatorSource::Proposal);
        assert_eq!(
            coordinator
                .raw_by_hash(&hash)
                .expect("raw payload")
                .tx
                .witness_hash(),
            proposal_witness,
            "a weaker duplicate cannot replace the authoritative witness"
        );
    });
    assert!(!runtime.is_failed());
    assert!(!shutdown.is_cancelled());
}

#[test]
fn retryable_capacity_classification_excludes_fixed_payload_limits() {
    use crate::component::pipeline_coordinator::CoordinatorError;

    assert!(
        CoordinatorError::ParentFanoutLimitExceeded(ckb_types::packed::Byte32::zero())
            .is_retryable_capacity_rejection()
    );
    assert!(CoordinatorError::GlobalBudgetExceeded.is_retryable_capacity_rejection());
    assert!(CoordinatorError::DependencyLimitExceeded.is_capacity_rejection());
    assert!(CoordinatorError::ConflictInputLimitExceeded.is_capacity_rejection());
    assert!(CoordinatorError::ResidencyChargeOverflow.is_capacity_rejection());
    assert!(
        !CoordinatorError::DependencyLimitExceeded.is_retryable_capacity_rejection(),
        "an identical payload cannot retry its way below a fixed dependency limit"
    );
    assert!(!CoordinatorError::ConflictInputLimitExceeded.is_retryable_capacity_rejection());
    assert!(!CoordinatorError::ResidencyChargeOverflow.is_retryable_capacity_rejection());
}

#[test]
fn rejected_commit_terminal_failure_is_fail_closed_not_best_effort() {
    use crate::component::entry::resolved_transaction_charge_bytes;
    use crate::component::pipeline_coordinator::{
        CoordinatorFeeGate, RawStage, TerminalDisposition, VerifySchedule, WorkerCapability,
    };
    use crate::component::pipeline_runtime::PipelineVerifiedTx;
    use crate::resolved_tx::ResolvedTx;
    use ckb_types::core::cell::ResolvedTransaction;
    use std::collections::HashSet;
    use std::time::Instant;

    let (consensus, out_points) = test_consensus(1);
    let (_store, snapshot) = snapshot_with_genesis(Arc::new(consensus.clone()));
    let shutdown = CancellationToken::new();
    let runtime = crate::component::pipeline_runtime::PipelineRuntime::new(
        &tx_pool_config(),
        &consensus,
        shutdown.clone(),
    );
    let tx = build_tx(&out_points[0], 4_000);
    let hash = tx.hash();
    runtime
        .admit_transaction(tx.clone(), TxSource::Local, 0, RawStage::PreCheck)
        .unwrap();
    let raw_lease = runtime.checkout_raw(RawStage::PreCheck).unwrap();
    let rtx = Arc::new(ResolvedTransaction::dummy_resolve(tx.clone()));
    let tx_size = tx.data().serialized_size_in_block();
    let resident_size = resolved_transaction_charge_bytes(tx_size, &rtx);
    let resolved = ResolvedTx {
        tx: tx.clone(),
        rtx,
        status: Status::Pending,
        fee: Capacity::zero(),
        tx_size,
        resident_size,
        pre_resolve_tip: snapshot.tip_hash(),
        source: TxSource::Local,
        epoch: 0,
    };
    runtime
        .mutate(|coordinator| {
            coordinator.complete_raw(
                &raw_lease,
                resolved,
                resident_size,
                VerifySchedule::default(),
            )
        })
        .unwrap();
    let verify_lease = runtime
        .mutate(|coordinator| coordinator.checkout_verify(WorkerCapability::Any))
        .unwrap()
        .unwrap();
    let candidate = (*verify_lease.payload).clone().into_pool_candidate();
    let candidate_charge = candidate.resident_size;
    let meta = CoordinatorFeeGate::new(0, 0)
        .validate(
            hash.clone(),
            tx.input_pts_iter().collect::<HashSet<_>>(),
            0,
            tx_size,
        )
        .unwrap();
    runtime
        .mutate(|coordinator| {
            coordinator.complete_verification_candidate(
                &verify_lease,
                PipelineVerifiedTx {
                    candidate,
                    completed: Completed {
                        cycles: 0,
                        fee: Capacity::zero(),
                    },
                    verify_cache_hit: false,
                    started_at: Instant::now(),
                },
                candidate_charge,
                meta,
            )
        })
        .unwrap();
    let commit = runtime.mutate_required("test commit checkout", |coordinator| {
        coordinator.begin_next_commit()
    });
    let commit = commit.unwrap();

    // Inject the report's closest version-mismatch leaf after checkout. The
    // coordinator transaction correctly leaves the entry Committing on Err;
    // production policy must therefore stop the service instead of warning
    // and leaking the active slot indefinitely.
    runtime.mutate(|coordinator| {
        coordinator
            .set_revision_for_test(&hash, commit.version.revision + 1)
            .unwrap();
    });
    let failure = catch_unwind(AssertUnwindSafe(|| {
        runtime.mutate_required(
            "rejected pipeline commit could not leave Committing",
            |state| state.fail_commit(&commit, TerminalDisposition::Rejected),
        );
    }));
    assert!(failure.is_err());
    assert!(runtime.is_failed());
    assert!(shutdown.is_cancelled());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pool_commit_panic_fails_closed_instead_of_stranding_committing() {
    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline(1);
    let tx = build_tx(&issue_out_points[0], 4_000);
    let cycles = measured_cycles(&service, tx.clone()).await;
    service
        .pool
        .tx_pool
        .write()
        .await
        .fail_next_pool_commit_panic = true;

    service
        .submit_remote_tx(
            tx,
            TxSource::Remote {
                cycles,
                peer: 1.into(),
            },
        )
        .await
        .expect("fault-injected transaction should reach the asynchronous pipeline");

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if service.pipeline.runtime.is_failed() && signal.is_cancelled() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("an authoritative pool panic must stop the complete tx-pool service");
}

pub(crate) fn tx_pool_config() -> TxPoolConfig {
    TxPoolConfig {
        max_tx_pool_size: 180_000_000,
        max_tx_pool_resident_size: 1_000_000_000,
        min_fee_rate: ckb_types::core::FeeRate::zero(),
        min_rbf_rate: ckb_types::core::FeeRate::zero(),
        max_tx_verify_cycles: MAX_TX_VERIFY_CYCLES,
        max_tx_verify_workers: 2,
        max_ancestors_count: 125,
        keep_rejected_tx_hashes_days: 1,
        keep_rejected_tx_hashes_count: 1000,
        persisted_data: Default::default(),
        recent_reject: Default::default(),
        expiry_hours: 24,
        verify_ordering: VerifyOrdering::ArrivalTime,
        max_tx_pipeline_resident_size: 384_000_000,
    }
}

pub(crate) fn test_consensus(issue_outputs: usize) -> (Consensus, Vec<OutPoint>) {
    let (always_success_cell, always_success_data, always_success_script) = always_success_cell();

    let always_success_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .output_data(always_success_data)
        .witness(always_success_script.clone().into_witness())
        .build();

    let issue_output = CellOutput::new_builder()
        .capacity(Capacity::bytes(ISSUE_OUTPUT_CAPACITY as usize).unwrap())
        .lock(always_success_script.clone())
        .build();
    let issue_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .outputs((0..issue_outputs).map(|_| issue_output.clone()))
        .outputs_data((0..issue_outputs).map(|_| Bytes::default().pack()))
        .build();

    let issue_out_points: Vec<_> = (0..issue_outputs)
        .map(|i| OutPoint::new(issue_tx.hash(), i as u32))
        .collect();

    let dao = ckb_dao_utils::genesis_dao_data(vec![&always_success_tx, &issue_tx]).unwrap();
    let genesis = BlockBuilder::default()
        .timestamp(1_557_310_743u64)
        .compact_target(difficulty_to_compact(U256::from(1000u64)))
        .dao(dao)
        .transaction(always_success_tx)
        .transaction(issue_tx)
        .build();

    let consensus = ConsensusBuilder::default()
        .genesis_block(genesis)
        .cellbase_maturity(EpochNumberWithFraction::new(0, 0, 1))
        .build();

    (consensus, issue_out_points)
}

pub(crate) fn snapshot_with_genesis(consensus: Arc<Consensus>) -> (MockStore, Arc<Snapshot>) {
    let store = MockStore::default();
    let genesis = consensus.genesis_block();
    let epoch_ext = consensus.genesis_epoch_ext().clone();

    {
        let db_txn = store.store().begin_transaction();
        let last_block_hash_in_previous_epoch = epoch_ext.last_block_hash_in_previous_epoch();
        db_txn.insert_block(genesis).unwrap();
        db_txn.attach_block(genesis).unwrap();
        attach_block_cell(&db_txn, genesis).unwrap();
        db_txn
            .insert_block_epoch_index(&genesis.hash(), &last_block_hash_in_previous_epoch)
            .unwrap();
        db_txn
            .insert_epoch_ext(&last_block_hash_in_previous_epoch, &epoch_ext)
            .unwrap();
        db_txn
            .insert_block_ext(
                &genesis.hash(),
                &BlockExt {
                    received_at: 0,
                    total_difficulty: U256::zero(),
                    total_uncles_count: 0,
                    verified: Some(true),
                    txs_fees: vec![],
                    cycles: None,
                    txs_sizes: None,
                },
            )
            .unwrap();
        let mut mmr = ChainRootMMR::new(0, &db_txn);
        mmr.push(genesis.digest()).unwrap();
        mmr.commit().unwrap();
        db_txn.commit().unwrap();
    }

    let snap = Arc::new(Snapshot::new(
        genesis.header(),
        U256::zero(),
        epoch_ext,
        store.store().get_snapshot(),
        Default::default(),
        consensus,
    ));
    (store, snap)
}

fn always_success_script() -> ckb_types::packed::Script {
    always_success_cell().2.clone()
}

fn always_success_dep() -> CellDep {
    CellDep::new_builder()
        .out_point(ckb_test_chain_utils::create_always_success_out_point())
        .build()
}

fn service_with_pipeline(
    issue_outputs: usize,
) -> (
    TxPoolService,
    ckb_channel::Receiver<crate::service::TxVerificationResult>,
    CancellationToken,
    MockStore,
    Vec<OutPoint>,
) {
    let h = super::harness::harness(issue_outputs).build();
    (h.service, h.relay_rx, h.cancel, h.store, h.out_points)
}

fn service_with_pipeline_workers(
    issue_outputs: usize,
    max_workers: usize,
) -> (
    TxPoolService,
    ckb_channel::Receiver<crate::service::TxVerificationResult>,
    CancellationToken,
    MockStore,
    Vec<OutPoint>,
) {
    let h = super::harness::harness(issue_outputs)
        .max_workers(max_workers)
        .build();
    (h.service, h.relay_rx, h.cancel, h.store, h.out_points)
}

fn build_tx(input: &OutPoint, output_capacity: usize) -> TransactionView {
    TransactionBuilder::default()
        .cell_dep(always_success_dep())
        .input(CellInput::new(input.clone(), 0))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(output_capacity).unwrap())
                .lock(always_success_script())
                .build(),
        )
        .output_data(Bytes::default().pack())
        .build()
}

fn with_cached_hash(tx: TransactionView, hash: ckb_types::packed::Byte32) -> TransactionView {
    ckb_types::packed::TransactionView::new_builder()
        .data(tx.data())
        .hash(hash)
        .witness_hash(tx.witness_hash())
        .build()
        .unpack()
}

/// A proposal short ID identifies only one protocol slot, not transaction
/// equality. Reporting a distinct colliding remote transaction as `Duplicated`
/// suppresses the relayer Reject terminal and leaves its filter resident. The
/// admission adapter must expose retryable backpressure and settle it once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pool_short_id_collision_is_not_a_successful_duplicate() {
    use super::harness::{WorkerSet, harness};
    use crate::component::entry::TxEntry;
    use crate::service::TxVerificationResult;

    let h = harness(2).workers(WorkerSet::None).build();
    let mut accepted_hash = [0x24; 32];
    let mut incoming_hash = accepted_hash;
    accepted_hash[31] = 1;
    incoming_hash[31] = 2;
    let accepted = with_cached_hash(
        build_tx(&h.out_points[0], 4_000),
        ckb_types::packed::Byte32::new(accepted_hash),
    );
    let incoming = with_cached_hash(
        build_tx(&h.out_points[1], 3_000),
        ckb_types::packed::Byte32::new(incoming_hash),
    );
    assert_eq!(accepted.proposal_short_id(), incoming.proposal_short_id());
    assert_ne!(accepted.hash(), incoming.hash());
    let incoming_hash = incoming.hash();
    let accepted_id = accepted.proposal_short_id();
    h.service
        .pool
        .tx_pool
        .write()
        .await
        .pool_map
        .add_entry(
            TxEntry::dummy_resolve(accepted, 0, Capacity::zero(), 100),
            Status::Pending,
        )
        .unwrap();

    let peer = ckb_network::PeerIndex::from(29);
    let reject = h
        .service
        .submit_remote_tx(incoming, TxSource::Remote { cycles: 0, peer })
        .await
        .expect_err("the occupied proposal slot must reject the distinct hash");
    assert!(matches!(reject, crate::error::Reject::Full(_)));
    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .get_pool_entry(&accepted_id)
            .is_some()
    );
    assert!(
        !h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&incoming_hash))
    );

    let relayed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = h.relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the rejected remote ingress must release its relayer filter");
    assert!(matches!(
        relayed,
        TxVerificationResult::Reject { tx_hash } if tx_hash == incoming_hash
    ));

    h.cancel.cancel();
}

/// Synchronous Local/reorg submissions use `pre_check` before the shared
/// authoritative commit. They must apply the same full-hash identity rule as
/// remote admission: an occupied short-id slot is retryable backpressure, not
/// evidence that the distinct transaction was already accepted.
#[tokio::test]
async fn synchronous_precheck_does_not_alias_short_id_collision_as_duplicate() {
    use super::harness::{WorkerSet, harness};
    use crate::component::entry::TxEntry;

    let h = harness(2).workers(WorkerSet::None).build();
    let mut accepted_hash = [0x25; 32];
    let mut incoming_hash = accepted_hash;
    accepted_hash[31] = 1;
    incoming_hash[31] = 2;
    let accepted = with_cached_hash(
        build_tx(&h.out_points[0], 4_000),
        ckb_types::packed::Byte32::new(accepted_hash),
    );
    let incoming = with_cached_hash(
        build_tx(&h.out_points[1], 3_000),
        ckb_types::packed::Byte32::new(incoming_hash),
    );
    assert_eq!(accepted.proposal_short_id(), incoming.proposal_short_id());
    assert_ne!(accepted.hash(), incoming.hash());
    h.service
        .pool
        .tx_pool
        .write()
        .await
        .pool_map
        .add_entry(
            TxEntry::dummy_resolve(accepted, 0, Capacity::zero(), 100),
            Status::Pending,
        )
        .unwrap();

    let tx_size = incoming.data().serialized_size_in_block();
    let (result, _) = h.service.pre_check(&incoming, tx_size).await;
    assert!(matches!(result, Err(crate::error::Reject::Full(_))));

    h.cancel.cancel();
}

async fn measured_cycles(service: &TxPoolService, tx: TransactionView) -> u64 {
    service
        .test_accept_tx(tx)
        .await
        .expect("local test accept should succeed")
        .cycles
}

// -----------------------------------------------------------------------------
// secp256k1 1-in-1-out helpers
// -----------------------------------------------------------------------------

const SECP_PRIVKEY: H256 =
    h256!("0xb2b3324cece882bca684eaf202667bb56ed8e8c2fd4b4dc71f615ebd6d9055a5");
const SECP_PUBKEY_HASH: H160 = h160!("0x779e5930892a0a9bf2fedfe048f685466c7d0396");
// 50,000 CKB and 1,000 CKB expressed in shannons (1 CKB = 10^8 shannons).
const SECP_ISSUE_CAPACITY: u64 = 50_000 * 100_000_000;
const SECP_FEE: u64 = 1_000 * 100_000_000;

fn secp_script() -> Script {
    let raw_data = BUNDLED_CELL
        .get("specs/cells/secp256k1_blake160_sighash_all")
        .expect("load secp256k1_blake160_sighash_all");
    let data: Bytes = raw_data.to_vec().into();
    Script::new_builder()
        .code_hash(CellOutput::calc_data_hash(&data))
        .args(Bytes::from(SECP_PUBKEY_HASH.as_bytes()))
        .hash_type(ckb_types::core::ScriptHashType::Data)
        .build()
}

fn secp_data_cell() -> (CellOutput, Bytes) {
    let raw_data = BUNDLED_CELL
        .get("specs/cells/secp256k1_data")
        .expect("load secp256k1_data");
    let data: Bytes = raw_data.to_vec().into();
    let cell = CellOutput::new_builder()
        .capacity(Capacity::bytes(data.len()).unwrap())
        .build();
    (cell, data)
}

fn secp_code_cell() -> (CellOutput, Bytes) {
    let raw_data = BUNDLED_CELL
        .get("specs/cells/secp256k1_blake160_sighash_all")
        .expect("load secp256k1_blake160_sighash_all");
    let data: Bytes = raw_data.to_vec().into();
    let cell = CellOutput::new_builder()
        .capacity(Capacity::bytes(data.len()).unwrap())
        .build();
    (cell, data)
}

fn create_secp_system_tx() -> TransactionView {
    let (code_cell, code_data) = secp_code_cell();
    let (data_cell, data_data) = secp_data_cell();
    let (_, _, script) = (code_cell.clone(), code_data.clone(), secp_script());
    TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .outputs(vec![code_cell, data_cell])
        .outputs_data(vec![code_data.pack(), data_data.pack()])
        .witness(script.into_witness())
        .build()
}

fn secp_cell_deps(system_tx: &TransactionView) -> Vec<CellDep> {
    vec![
        CellDep::new_builder()
            .out_point(OutPoint::new(system_tx.hash(), 0))
            .build(),
        CellDep::new_builder()
            .out_point(OutPoint::new(system_tx.hash(), 1))
            .build(),
    ]
}

pub(crate) fn secp_test_consensus(
    issue_outputs: usize,
) -> (Consensus, Vec<OutPoint>, Vec<CellDep>) {
    let system_tx = create_secp_system_tx();
    let cell_deps = secp_cell_deps(&system_tx);
    let lock = secp_script();

    let issue_output = CellOutput::new_builder()
        .capacity(Capacity::shannons(SECP_ISSUE_CAPACITY))
        .lock(lock)
        .build();
    let issue_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .outputs((0..issue_outputs).map(|_| issue_output.clone()))
        .outputs_data((0..issue_outputs).map(|_| Bytes::default().pack()))
        .build();

    let issue_out_points: Vec<_> = (0..issue_outputs)
        .map(|i| OutPoint::new(issue_tx.hash(), i as u32))
        .collect();

    let dao = ckb_dao_utils::genesis_dao_data(vec![&system_tx, &issue_tx]).unwrap();
    let genesis = BlockBuilder::default()
        .timestamp(1_557_310_743u64)
        .compact_target(difficulty_to_compact(U256::from(1000u64)))
        .dao(dao)
        .transaction(system_tx)
        .transaction(issue_tx)
        .build();

    let consensus = ConsensusBuilder::default()
        .genesis_block(genesis)
        .cellbase_maturity(EpochNumberWithFraction::new(0, 0, 1))
        .build();

    (consensus, issue_out_points, cell_deps)
}

fn secp_service_with_pipeline_workers(
    issue_outputs: usize,
    max_workers: usize,
) -> (
    TxPoolService,
    ckb_channel::Receiver<crate::service::TxVerificationResult>,
    CancellationToken,
    MockStore,
    Vec<OutPoint>,
    Vec<CellDep>,
) {
    let h = super::harness::harness(issue_outputs)
        .secp(true)
        .max_workers(max_workers)
        .build();
    (
        h.service,
        h.relay_rx,
        h.cancel,
        h.store,
        h.out_points,
        h.cell_deps.expect("secp harness provides cell deps"),
    )
}

fn build_secp_tx(input: &OutPoint, cell_deps: &[CellDep], output_capacity: u64) -> TransactionView {
    let lock = secp_script();
    let output = CellOutput::new_builder()
        .capacity(Capacity::shannons(output_capacity))
        .lock(lock)
        .build();

    let raw = TransactionBuilder::default()
        .inputs(vec![CellInput::new(input.clone(), 0)])
        .cell_deps(cell_deps.iter().cloned())
        .output(output)
        .output_data(Bytes::default().pack())
        .build();

    let privkey: Privkey = SECP_PRIVKEY.into();
    let witness_placeholder = WitnessArgs::new_builder()
        .lock(Some(Bytes::from(vec![0u8; 65])))
        .build();
    let witness_len: u64 = witness_placeholder.as_bytes().len() as u64;

    let mut blake2b = ckb_hash::new_blake2b();
    let mut message = [0u8; 32];
    blake2b.update(&raw.hash().raw_data()[..]);
    blake2b.update(&witness_len.to_le_bytes());
    blake2b.update(&witness_placeholder.as_bytes());
    blake2b.finalize(&mut message);

    let message = H256::from(message);
    let sig: Bytes = privkey
        .sign_recoverable(&message)
        .expect("sign tx")
        .serialize()
        .into();
    let witness = witness_placeholder.as_builder().lock(Some(sig)).build();

    raw.as_advanced_builder()
        .set_witnesses(vec![witness.as_bytes().into()])
        .build()
}

async fn submit_local_tx(service: &TxPoolService, tx: TransactionView) -> u64 {
    service
        .process_tx_direct(tx, TxSource::Local, None)
        .await
        .expect("local tx should be accepted")
        .cycles
}

async fn verify_cycles(service: &TxPoolService, tx: TransactionView) -> u64 {
    let tx_size = tx.data().serialized_size_in_block();
    let (pre_check_ret, snapshot) = service.pre_check(&tx, tx_size).await;
    let PreCheckedTx { rtx, status, .. } =
        pre_check_ret.expect("pre_check for cycle measurement should succeed");
    let verify_cache = service.fetch_tx_verify_cache(&tx).await;
    let max_cycles = service.pool.consensus.max_block_cycles();
    let tx_env = match status {
        Status::Pending => Arc::new(TxVerifyEnv::new_submit(snapshot.tip_header())),
        Status::Gap => Arc::new(TxVerifyEnv::new_proposed(snapshot.tip_header(), 0)),
        Status::Proposed => Arc::new(TxVerifyEnv::new_proposed(snapshot.tip_header(), 1)),
    };
    let verified = crate::util::verify_rtx(
        Arc::clone(&snapshot),
        Arc::clone(&rtx),
        tx_env,
        &verify_cache,
        max_cycles,
        None,
    )
    .await
    .expect("verify_rtx for cycle measurement should succeed");
    verified.cycles
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verification_cache_isolated_by_witness_hash_not_raw_hash() {
    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline(1);
    let raw = build_tx(&issue_out_points[0], 4_000);
    let first = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"first").pack()])
        .build();
    let second = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"second").pack()])
        .build();
    assert_eq!(first.hash(), second.hash());
    assert_ne!(first.witness_hash(), second.witness_hash());

    let cached = Completed {
        cycles: 42,
        fee: Capacity::shannons(7),
    };
    service
        .aux
        .txs_verify_cache
        .write()
        .await
        .put(TxVerificationCacheKey::from_transaction(&first), cached);

    assert_eq!(service.fetch_tx_verify_cache(&first).await, Some(cached));
    assert_eq!(service.fetch_tx_verify_cache(&second).await, None);
    signal.cancel();
}

/// Detached-transaction recovery must query the verification cache with the
/// exact witness-bearing transaction being recovered.  The historical
/// `readd_detached_tx` path built a map by witness hash but looked it up by raw
/// transaction hash, turning every recovery into an avoidable cache miss.
/// This also guards against the more dangerous inverse error: reusing a cache
/// entry produced for another witness variant with the same raw hash.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reorg_recovery_reads_cache_by_exact_witness_hash() {
    use std::collections::{HashSet, VecDeque};

    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline(2);
    let exact = build_tx(&issue_out_points[0], 4_000)
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"exact").pack()])
        .build();
    let other_raw = build_tx(&issue_out_points[1], 4_000);
    let cached_variant = other_raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"cached-variant").pack()])
        .build();
    let recovered_variant = other_raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"recovered-variant").pack()])
        .build();
    assert_eq!(cached_variant.hash(), recovered_variant.hash());
    assert_ne!(
        cached_variant.witness_hash(),
        recovered_variant.witness_hash()
    );

    let exact_measured = verify_cycles(&service, exact.clone()).await;
    let recovered_measured = verify_cycles(&service, recovered_variant.clone()).await;
    let exact_cached = exact_measured + 17;
    let wrong_variant_cached = recovered_measured + 29;
    assert!(wrong_variant_cached < MAX_TX_VERIFY_CYCLES);

    {
        let mut cache = service.aux.txs_verify_cache.write().await;
        cache.put(
            TxVerificationCacheKey::from_transaction(&exact),
            Completed {
                cycles: exact_cached,
                fee: Capacity::shannons(0),
            },
        );
        cache.put(
            TxVerificationCacheKey::from_transaction(&cached_variant),
            Completed {
                cycles: wrong_variant_cached,
                fee: Capacity::shannons(0),
            },
        );
    }

    let detached_block = BlockBuilder::default()
        .number(1)
        .parent_hash(service.pool.tx_pool.read().await.snapshot.tip_hash())
        .epoch(EpochNumberWithFraction::new(0, 0, 1).full_value())
        .transaction(
            TransactionBuilder::default()
                .input(CellInput::new(OutPoint::null(), 0))
                .output(
                    CellOutput::new_builder()
                        .capacity(Capacity::bytes(1_000).unwrap())
                        .build(),
                )
                .output_data(Bytes::default().pack())
                .build(),
        )
        .transaction(exact.clone())
        .transaction(recovered_variant.clone())
        .build();
    let snapshot = service.pool.tx_pool.read().await.cloned_snapshot();

    service
        .update_tx_pool_for_reorg(
            VecDeque::from([detached_block]),
            VecDeque::new(),
            HashSet::new(),
            snapshot,
        )
        .await
        .unwrap();

    let pool = service.pool.tx_pool.read().await;
    let exact_entry = pool
        .get_pool_entry(&exact.proposal_short_id())
        .expect("exact cached detached transaction is recovered");
    assert_eq!(
        exact_entry.inner.cycles, exact_cached,
        "detached recovery must hit the exact witness-hash cache entry"
    );
    let recovered_entry = pool
        .get_pool_entry(&recovered_variant.proposal_short_id())
        .expect("uncached witness variant is recovered");
    assert_eq!(
        recovered_entry.inner.cycles, recovered_measured,
        "a cache entry for another witness variant must not be reused"
    );
    drop(pool);

    signal.cancel();
}

/// Script verification needs complete expanded cell deps, but retaining their
/// attacker-controlled outputs/data after verification turns a compact
/// dep-group reference into an accepted-pool memory multiplier. The typed
/// verified-to-candidate transition strips that verification-only payload,
/// preserves the dep identity/maturity metadata, and retains complete inputs
/// for DAO accounting. The accepted-pool resident budget still accounts for
/// the payload that must remain.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verified_candidate_compacts_deps_and_pool_budget_counts_retained_inputs() {
    use super::harness::{WorkerSet, harness};
    use crate::component::entry::{
        TxEntry, accepted_transaction_charge_bytes, resolved_transaction_charge_bytes,
    };
    use crate::resolved_tx::ResolvedTx;
    use ckb_types::core::TransactionInfo;
    use ckb_types::core::cell::{CellMetaBuilder, ResolvedTransaction};

    let h = harness(1).workers(WorkerSet::None).build();
    let transaction = build_tx(&h.out_points[0], 4_000);
    let tx_size = transaction.data().serialized_size_in_block();
    let retained_input_data = Bytes::from(vec![0x5a; 1_000_000]);
    let dep_data = Bytes::from(vec![0xa5; 1_000_000]);
    let dep_out_point = OutPoint::new(Default::default(), 7);
    let dep_transaction_info = TransactionInfo::new(
        1,
        EpochNumberWithFraction::new(0, 0, 1),
        Default::default(),
        0,
    );
    let resolved = Arc::new(ResolvedTransaction {
        transaction: transaction.clone(),
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder().build(),
                retained_input_data,
            )
            .out_point(h.out_points[0].clone())
            .build(),
        ],
        resolved_cell_deps: vec![
            CellMetaBuilder::from_cell_output(CellOutput::new_builder().build(), dep_data)
                .out_point(dep_out_point.clone())
                .transaction_info(dep_transaction_info.clone())
                .build(),
        ],
        resolved_dep_groups: Vec::new(),
    });
    let full_resident_size = resolved_transaction_charge_bytes(tx_size, &resolved);
    assert!(full_resident_size > tx_size + 2_000_000);
    let snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();
    let candidate = ResolvedTx {
        tx: transaction,
        rtx: resolved,
        status: Status::Pending,
        fee: Capacity::shannons(1000),
        tx_size,
        resident_size: full_resident_size,
        pre_resolve_tip: snapshot.tip_hash(),
        source: TxSource::Local,
        epoch: 0,
    }
    .into_pool_candidate();

    let dep = &candidate.rtx.resolved_cell_deps[0];
    assert_eq!(dep.out_point, dep_out_point);
    assert_eq!(dep.transaction_info, Some(dep_transaction_info));
    assert_eq!(dep.cell_output, CellOutput::default());
    assert_eq!(dep.data_bytes, 0);
    assert!(dep.mem_cell_data.is_none());
    assert!(dep.mem_cell_data_hash.is_none());
    assert_eq!(
        candidate.rtx.resolved_inputs[0]
            .mem_cell_data
            .as_ref()
            .map(Bytes::len),
        Some(1_000_000),
        "DAO-relevant input data must remain available"
    );
    assert!(candidate.resident_size > tx_size + 1_000_000);
    assert!(candidate.resident_size < full_resident_size - 900_000);
    assert_eq!(
        candidate.resident_size,
        accepted_transaction_charge_bytes(tx_size, &candidate.rtx),
        "verified handoff must reserve accepted payload and index residency"
    );
    assert!(
        candidate.resident_size > resolved_transaction_charge_bytes(tx_size, &candidate.rtx),
        "accepted-state indexes must not be omitted from the resident budget"
    );

    let entry = TxEntry::new_with_resident_size(
        candidate.rtx,
        42,
        candidate.fee,
        tx_size,
        candidate.resident_size,
    );
    let resident_size = entry.resident_size();
    let id = entry.proposal_short_id();

    let mut pool = h.service.pool.tx_pool.write().await;
    pool.config.max_tx_pool_size = tx_size;
    pool.config.max_tx_pool_resident_size = resident_size - 1;
    pool.add_pending(entry)
        .expect("entry inserts before limits run");
    assert_eq!(pool.pool_map.stats.total_tx_size, tx_size);
    assert_eq!(pool.pool_map.stats.total_tx_resident_size, resident_size);

    let mut rejects = Vec::new();
    let reject = pool.limit_size(Some(&id), &mut rejects);
    assert!(matches!(reject, Some(crate::error::Reject::Full(_))));
    assert!(pool.get_pool_entry(&id).is_none());
    assert_eq!(pool.pool_map.stats.total_tx_size, 0);
    assert_eq!(pool.pool_map.stats.total_tx_resident_size, 0);
    assert_eq!(rejects.len(), 1);
    drop(pool);

    h.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn limit_size_repairs_high_counter_drift_before_returning() {
    use super::harness::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let mut pool = h.service.pool.tx_pool.write().await;
    let serialized_drift = pool.config.max_tx_pool_size.saturating_add(1);
    let resident_drift = pool.config.tx_pool_resident_size_budget().saturating_add(1);
    pool.pool_map.stats.total_tx_size = serialized_drift;
    pool.pool_map.stats.total_tx_resident_size = resident_drift;

    let mut rejects = Vec::new();
    assert!(pool.limit_size(None, &mut rejects).is_none());
    assert!(rejects.is_empty());
    assert_eq!(pool.pool_map.stats.total_tx_size, 0);
    assert_eq!(pool.pool_map.stats.total_tx_resident_size, 0);
    assert_eq!(pool.pool_map.stats.total_tx_cycles, 0);
    drop(pool);
    h.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_processes_independent_remote_txs() {
    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline(5);

    let txs: Vec<_> = issue_out_points
        .iter()
        .map(|out_point| build_tx(out_point, 4_000))
        .collect();

    for tx in &txs {
        let cycles = measured_cycles(&service, tx.clone()).await;
        service
            .submit_remote_tx(
                tx.clone(),
                TxSource::Remote {
                    cycles,
                    peer: 1.into(),
                },
            )
            .await
            .expect("enqueue remote tx should succeed");
    }

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == txs.len() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("pipeline should process all independent txs in time");

    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(pending, txs.len());

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// Local RPC submission is intentionally synchronous. An older asynchronous
/// remote owner for the same hash must neither turn the local call into a
/// duplicate error nor survive after the local transaction enters TxPool.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_submit_bypasses_and_settles_matching_remote_owner() {
    use super::harness::{WorkerSet, harness};
    use crate::service::TxVerificationResult;

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    let id = tx.proposal_short_id();
    let peer = ckb_network::PeerIndex::from(1);

    h.service
        .submit_remote_tx(tx.clone(), TxSource::Remote { cycles: 0, peer })
        .await
        .expect("remote copy enters the coordinator");
    assert!(
        h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&hash)),
        "the no-worker harness must leave the remote copy coordinator-owned"
    );

    let completed = h
        .service
        .process_tx(tx, TxSource::Local)
        .await
        .expect("local submission must execute synchronously");
    assert!(completed.cycles > 0);
    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .get_tx_from_pool(&id)
            .is_some(),
        "the local call must return only after authoritative pool insertion"
    );
    assert!(
        !h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&hash)),
        "successful local insertion must invalidate the older async owner"
    );
    let relayed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = h.relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("local handoff must settle the consumed remote ingress");
    assert!(matches!(
        relayed,
        TxVerificationResult::Ok {
            original_peer: Some(relayed_peer),
            tx_hash,
        } if relayed_peer == peer && tx_hash == hash
    ));

    h.cancel.cancel();
}

/// Candidate checkout is part of the authoritative TxPool write transaction.
/// If it happened before waiting for that guard, a synchronous Local/clear/
/// reorg handoff could consume the coordinator owner while the old driver was
/// already carrying a `Committing` lease. Its later failure settlement would
/// then confuse a legitimate stale lease with coordinator corruption.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pipeline_commit_worker_waits_for_the_pool_sequencer() {
    use super::harness::{WorkerSet, harness};
    use crate::component::pipeline_coordinator::{
        CoordinatorFeeGate, CoordinatorLocation, RawStage, WorkerCapability,
    };
    use crate::component::pipeline_runtime::candidate_charge_bytes;
    use std::collections::HashSet;

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    let cycles = measured_cycles(&h.service, tx.clone()).await;
    h.service
        .submit_remote_tx(
            tx.clone(),
            TxSource::Remote {
                cycles,
                peer: 1.into(),
            },
        )
        .await
        .unwrap();

    let raw = h
        .service
        .pipeline
        .runtime
        .checkout_raw(RawStage::PreCheck)
        .unwrap();
    h.service.process_pipeline_raw_lease(raw).await;
    let verify = h
        .service
        .pipeline
        .runtime
        .mutate(|coordinator| coordinator.checkout_verify(WorkerCapability::Any))
        .unwrap()
        .unwrap();
    let snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();
    let verified = h
        .service
        .verify_pipeline_resolved((*verify.payload).clone(), snapshot, None)
        .await
        .unwrap();
    let candidate = CoordinatorFeeGate::new(0, 0)
        .validate(
            hash.clone(),
            tx.input_pts_iter().collect::<HashSet<_>>(),
            verified.candidate.fee.as_u64(),
            verified.candidate.tx_size,
        )
        .unwrap();
    let charge = candidate_charge_bytes(&verified.candidate)
        .unwrap()
        .checked_add(std::mem::size_of::<
            crate::component::pipeline_runtime::PipelineVerifiedTx,
        >())
        .unwrap();
    h.service
        .pipeline
        .runtime
        .mutate(|coordinator| {
            coordinator.complete_verification_candidate(&verify, verified, charge, candidate)
        })
        .unwrap();
    assert_eq!(
        h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.view(&hash).unwrap().location),
        CoordinatorLocation::Verified
    );

    let pool_guard = h.service.pool.tx_pool.write().await;
    let commit_cancel = h.cancel.child_token();
    let service = h.service.clone();
    let driver = tokio::spawn(crate::service::workers::run_pipeline_commit_worker(
        service,
        commit_cancel,
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.view(&hash).unwrap().location),
        CoordinatorLocation::Verified,
        "waiting for TxPool must not publish a cancellable Committing lease"
    );
    assert!(!driver.is_finished());

    drop(pool_guard);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if h.service
                .pool
                .tx_pool
                .read()
                .await
                .get_tx_from_pool_by_hash(&hash)
                .is_some()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("commit worker resumes after the pool sequencer is released");
    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .get_tx_from_pool_by_hash(&hash)
            .is_some()
    );
    assert!(!h.service.pipeline.runtime.is_failed());
    h.cancel.cancel();
    tokio::time::timeout(Duration::from_secs(1), driver)
        .await
        .expect("commit worker observes cancellation")
        .expect("commit worker does not panic");
}

/// The early duplicate check can become stale before pipeline admission. The
/// authoritative admission boundary must recheck TxPool while holding its read
/// guard across the coordinator mutation, so a transaction committed in that
/// window is never shadowed by a second pre-pool owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_precheck_cannot_readmit_an_already_accepted_transaction() {
    use super::harness::{WorkerSet, harness};
    use crate::service::TxVerificationResult;

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    h.service
        .process_tx(tx.clone(), TxSource::Local)
        .await
        .expect("local transaction enters the authoritative pool");

    // Consume the local success publication before observing the synthetic
    // stale-precheck ingress below.
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = h.relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("local success is published");

    let peer = ckb_network::PeerIndex::from(17);
    assert!(
        !h.service
            .classify_and_enqueue_tx_spawn(tx, TxSource::Remote { cycles: 0, peer },)
            .await
            .expect("already accepted ingress settles without readmission")
    );
    assert!(
        !h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&hash)),
        "TxPool and coordinator must never both own the same hash"
    );

    let relayed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = h.relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the stale remote ingress receives a terminal settlement");
    assert!(matches!(
        relayed,
        TxVerificationResult::Ok {
            original_peer: Some(relayed_peer),
            tx_hash,
        } if relayed_peer == peer && tx_hash == hash
    ));

    h.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_unverified_remote_owner_is_not_acknowledged_as_accepted() {
    use super::harness::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let first = build_tx(&h.out_points[0], 4_000)
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"first").pack()])
        .build();
    let second = first
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"second").pack()])
        .build();
    assert_eq!(first.hash(), second.hash());
    assert_ne!(first.witness_hash(), second.witness_hash());

    h.service
        .submit_remote_tx(
            first,
            TxSource::Remote {
                cycles: 0,
                peer: 19.into(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        h.service
            .submit_remote_tx(
                second,
                TxSource::Remote {
                    cycles: 0,
                    peer: 20.into(),
                },
            )
            .await,
        Err(crate::error::Reject::Duplicated(_))
    ));
    tokio::task::yield_now().await;
    assert!(
        h.relay_rx.try_recv().is_err(),
        "a merely coordinator-owned raw hash has no successful result yet"
    );

    h.cancel.cancel();
}

/// Proposal notification upgrades an existing remote owner in place. The
/// old peer can then be banned without revoking the trusted transaction, and
/// a lease checked out before promotion must settle under the new source.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proposal_promotes_active_remote_owner_and_detaches_peer_ban() {
    use super::harness::{WorkerSet, harness};
    use crate::component::pipeline_coordinator::{
        CoordinatorLocation, CoordinatorSource, RawStage,
    };
    use crate::service::TxVerificationResult;

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    let peer = ckb_network::PeerIndex::from(7);
    h.service
        .submit_remote_tx(tx.clone(), TxSource::Remote { cycles: 0, peer })
        .await
        .unwrap();
    assert_eq!(
        h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.deadline_len()),
        1
    );
    let lease = h
        .service
        .pipeline
        .runtime
        .checkout_raw(RawStage::PreCheck)
        .unwrap();

    assert!(
        !h.service
            .notify_tx(tx)
            .await
            .expect("proposal promotes the existing hash")
    );
    assert_eq!(
        h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.view(&hash).unwrap().source),
        CoordinatorSource::Proposal
    );
    assert_eq!(
        h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.deadline_len()),
        0,
        "trusted promotion must cancel the obsolete remote expiry"
    );
    h.service
        .ban_malformed(peer, "test old remote owner ban".to_string())
        .await;
    h.service.process_pipeline_raw_lease(lease).await;
    let view = h
        .service
        .pipeline
        .runtime
        .read(|coordinator| coordinator.view(&hash).unwrap());
    assert_eq!(view.source, CoordinatorSource::Proposal);
    assert_eq!(view.location, CoordinatorLocation::VerifyQueued);

    let verify = h
        .service
        .pipeline
        .runtime
        .mutate(|coordinator| {
            coordinator
                .checkout_verify(crate::component::pipeline_coordinator::WorkerCapability::Any)
        })
        .unwrap()
        .unwrap();
    let mut chunk_rx = h.service.pipeline.chunk_rx.clone();
    h.service
        .process_pipeline_verify_lease(verify, &mut chunk_rx)
        .await;
    let relayed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = h.relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("promoted remote ingress receives one successful settlement");
    assert!(
        matches!(
            relayed,
            TxVerificationResult::Ok {
                original_peer: Some(relayed_peer),
                tx_hash,
            } if relayed_peer == peer && tx_hash == hash
        ),
        "trusted scheduling priority must not erase immutable relay attribution"
    );

    h.cancel.cancel();
}

/// Resolved work can wait behind expensive verification for many blocks. It
/// must not pin one RocksDB snapshot per historical tip while queued; a stale
/// resolution returns to the ordered resolver before script execution.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_resolved_work_is_snapshot_free_and_stale_tip_requeues() {
    use super::harness::{WorkerSet, harness};
    use crate::component::pipeline_coordinator::{CoordinatorLocation, RawStage, WorkerCapability};

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    let peer = ckb_network::PeerIndex::from(42);
    h.service
        .submit_remote_tx(tx, TxSource::Remote { cycles: 0, peer })
        .await
        .unwrap();
    let raw = h
        .service
        .pipeline
        .runtime
        .checkout_raw(RawStage::PreCheck)
        .unwrap();
    h.service.process_pipeline_raw_lease(raw).await;
    let verify = h
        .service
        .pipeline
        .runtime
        .mutate_required("test verify checkout", |coordinator| {
            coordinator.checkout_verify(WorkerCapability::Any)
        })
        .unwrap();

    let old_snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();
    let old_snapshot_weak = Arc::downgrade(&old_snapshot);
    let next_block = BlockBuilder::default()
        .parent_hash(old_snapshot.tip_hash())
        .number(old_snapshot.tip_number() + 1)
        .epoch(EpochNumberWithFraction::new(0, 0, 1))
        .build();
    let next_snapshot = Arc::new(Snapshot::new(
        next_block.header(),
        old_snapshot.total_difficulty().clone(),
        old_snapshot.epoch_ext().clone(),
        h.store.store().get_snapshot(),
        Default::default(),
        old_snapshot.cloned_consensus(),
    ));
    h.service.pool.tx_pool.write().await.snapshot = next_snapshot;
    drop(old_snapshot);
    assert!(
        old_snapshot_weak.upgrade().is_none(),
        "queued/active verification payload must not retain the old database snapshot"
    );

    let mut chunk_rx = h.service.pipeline.chunk_rx.clone();
    h.service
        .process_pipeline_verify_lease(verify, &mut chunk_rx)
        .await;
    assert_eq!(
        h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.view(&hash).unwrap().location),
        CoordinatorLocation::RawQueued(RawStage::Resolve)
    );
    h.cancel.cancel();
}

/// Raw hash equality is insufficient for source promotion because witnesses
/// remain verification inputs. A proposal carrying another witness variant
/// must atomically restart normal bounded processing with the trusted payload,
/// rather than synchronously verifying on the dispatcher or continuing an old
/// remote lease.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proposal_witness_variant_replaces_remote_payload_at_authoritative_handoff() {
    use super::harness::{WorkerSet, harness};
    use crate::component::pipeline_coordinator::{
        CoordinatorLocation, CoordinatorSource, RawStage,
    };
    use crate::service::TxVerificationResult;

    let h = harness(1).workers(WorkerSet::None).build();
    let remote = build_tx(&h.out_points[0], 4_000)
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"remote").pack()])
        .build();
    let proposal = remote
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"proposal").pack()])
        .build();
    assert_eq!(remote.hash(), proposal.hash());
    assert_ne!(remote.witness_hash(), proposal.witness_hash());
    let hash = remote.hash();
    let id = remote.proposal_short_id();
    let peer = ckb_network::PeerIndex::from(18);

    h.service
        .submit_remote_tx(remote, TxSource::Remote { cycles: 0, peer })
        .await
        .expect("remote witness variant enters coordinator");
    let old_lease = h
        .service
        .pipeline
        .runtime
        .checkout_raw(RawStage::PreCheck)
        .unwrap();
    assert!(!h.service.notify_tx(proposal.clone()).await.unwrap());

    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .get_tx_from_pool(&id)
            .is_none(),
        "proposal notification must not synchronously execute script verification"
    );
    let (view, payload_witness, ingress_peer, blame_peer) =
        h.service.pipeline.runtime.read(|coordinator| {
            let raw = coordinator.raw_by_hash(&hash).unwrap();
            (
                coordinator.view(&hash).unwrap(),
                raw.tx.witness_hash(),
                raw.ingress_peer(),
                raw.blame_peer(),
            )
        });
    assert_eq!(view.source, CoordinatorSource::Proposal);
    assert_eq!(
        view.location,
        CoordinatorLocation::RawQueued(RawStage::PreCheck)
    );
    assert_eq!(payload_witness, proposal.witness_hash());
    assert_eq!(ingress_peer, Some(peer));
    assert_eq!(
        blame_peer, None,
        "a trusted replacement witness must not blame its old ingress peer"
    );
    assert!(matches!(
        h.service
            .pipeline
            .runtime
            .mutate(|coordinator| coordinator.requeue_raw(&old_lease)),
        Err(crate::component::pipeline_coordinator::CoordinatorError::RevisionMismatch { .. })
    ));

    let replacement = h
        .service
        .pipeline
        .runtime
        .checkout_raw(RawStage::PreCheck)
        .unwrap();
    assert_eq!(
        replacement.payload.tx.witness_hash(),
        proposal.witness_hash()
    );
    h.service.process_pipeline_raw_lease(replacement).await;
    let verify = h
        .service
        .pipeline
        .runtime
        .mutate(|coordinator| {
            coordinator
                .checkout_verify(crate::component::pipeline_coordinator::WorkerCapability::Any)
        })
        .unwrap()
        .unwrap();
    let mut chunk_rx = h.service.pipeline.chunk_rx.clone();
    h.service
        .process_pipeline_verify_lease(verify, &mut chunk_rx)
        .await;

    let resident = h
        .service
        .pool
        .tx_pool
        .read()
        .await
        .get_tx_from_pool(&id)
        .cloned()
        .expect("trusted proposal variant commits");
    assert_eq!(resident.witness_hash(), proposal.witness_hash());
    assert!(
        !h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&hash))
    );
    let relayed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = h.relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the older remote ingress is settled by the trusted handoff");
    assert!(matches!(
        relayed,
        TxVerificationResult::Ok {
            original_peer: Some(relayed_peer),
            tx_hash,
        } if relayed_peer == peer && tx_hash == hash
    ));

    h.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proposal_promoted_remote_terminal_still_releases_ingress_filter() {
    use super::harness::{WorkerSet, harness};
    use crate::service::TxVerificationResult;

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    let peer = ckb_network::PeerIndex::from(8);
    h.service
        .submit_remote_tx(tx.clone(), TxSource::Remote { cycles: 0, peer })
        .await
        .unwrap();
    assert!(!h.service.notify_tx(tx).await.unwrap());

    h.service.clear_pipeline().await;
    let relayed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = h.relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("promoted remote ingress receives one terminal settlement");
    assert!(matches!(
        relayed,
        TxVerificationResult::Reject { tx_hash } if tx_hash == hash
    ));

    h.cancel.cancel();
}

/// A saturated external-effect budget must backpressure before the
/// authoritative pool mutation. Otherwise cancellation while waiting to
/// journal the callback/relay result could leave an accepted transaction with
/// no terminal publication record.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_commit_waits_for_effect_credit_before_mutating_pool() {
    use super::harness::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let tx_hash = tx.hash();
    let held = h
        .service
        .relay
        .effects
        .reserve(512_000_000)
        .await
        .expect("test owns the complete outbox byte budget");

    let service = h.service.clone();
    let submit = tokio::spawn(async move { service.process_tx(tx, TxSource::Local).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !h.service
            .pool
            .tx_pool
            .read()
            .await
            .pool_map
            .iter()
            .any(|entry| entry.inner.transaction().hash() == tx_hash),
        "pool membership must not change while effect preflight is blocked"
    );

    drop(held);
    tokio::time::timeout(Duration::from_secs(5), submit)
        .await
        .expect("submission resumes after effect credit is released")
        .expect("submission task joins")
        .expect("local transaction commits");
    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .pool_map
            .iter()
            .any(|entry| entry.inner.transaction().hash() == tx_hash)
    );

    h.cancel.cancel();
}

/// A parent can commit after a child resolver observed `Unknown` but before
/// it registers the wait. The atomic TxPool -> coordinator settlement must
/// requeue the child instead of installing a waiter after the only wake edge.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parent_commit_before_wait_registration_requeues_child() {
    use super::harness::{WorkerSet, harness};
    use crate::component::pipeline_coordinator::{CoordinatorLocation, RawStage};
    use crate::service::pipeline_ops::ParentWaitOutcome;
    use std::collections::HashSet;

    let h = harness(1).workers(WorkerSet::None).build();
    let parent = build_tx(&h.out_points[0], 4_000);
    let child = build_tx(&OutPoint::new(parent.hash(), 0), 3_000);
    let child_hash = child.hash();
    let epoch = h.service.current_pipeline_epoch().unwrap();
    h.service
        .pipeline
        .runtime
        .admit_transaction(
            child,
            TxSource::Remote {
                cycles: 0,
                peer: 1.into(),
            },
            epoch,
            RawStage::Resolve,
        )
        .unwrap();
    let lease = h
        .service
        .pipeline
        .runtime
        .checkout_raw(RawStage::Resolve)
        .unwrap();

    h.service
        .process_tx(parent.clone(), TxSource::Local)
        .await
        .expect("parent commits before child waiter registration");
    assert!(matches!(
        h.service
            .settle_raw_parent_wait(
                &lease,
                HashSet::from([parent.hash()]),
                h.service
                    .reserve_effects(TxPoolService::unknown_parents_effect_bytes(1))
                    .await
                    .unwrap(),
            )
            .await
            .unwrap(),
        ParentWaitOutcome::Requeued
    ));
    assert_eq!(
        h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.view(&child_hash).unwrap().location),
        CoordinatorLocation::RawQueued(RawStage::Resolve)
    );

    h.cancel.cancel();
}

/// A remote transaction becomes externally observable as `UnknownParents`
/// only through the same coordinator transition that installs its durable
/// parent wait. This guards against cancellation leaving either a silent
/// waiter or a parent request with no owned transaction behind it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_parent_wait_and_unknown_parents_effect_are_one_transition() {
    use super::harness::{WorkerSet, harness};
    use crate::component::pipeline_coordinator::{CoordinatorLocation, RawStage};
    use crate::service::TxVerificationResult;
    use crate::service::pipeline_ops::ParentWaitOutcome;
    use ckb_types::packed::Byte32;
    use std::collections::HashSet;

    let h = harness(0).workers(WorkerSet::None).build();
    let parent = Byte32::new([42; 32]);
    let child = build_tx(&OutPoint::new(parent.clone(), 0), 3_000);
    let child_hash = child.hash();
    let peer = 7.into();
    let epoch = h.service.current_pipeline_epoch().unwrap();
    h.service
        .pipeline
        .runtime
        .admit_transaction(
            child,
            TxSource::Remote { cycles: 0, peer },
            epoch,
            RawStage::Resolve,
        )
        .unwrap();
    let lease = h
        .service
        .pipeline
        .runtime
        .checkout_raw(RawStage::Resolve)
        .unwrap();

    let outcome = h
        .service
        .settle_raw_parent_wait(
            &lease,
            HashSet::from([parent.clone()]),
            h.service
                .reserve_effects(TxPoolService::unknown_parents_effect_bytes(1))
                .await
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(outcome, ParentWaitOutcome::Parked));
    assert_eq!(
        h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.view(&child_hash).unwrap().location),
        CoordinatorLocation::WaitingParents {
            missing: HashSet::from([parent.clone()])
        }
    );

    let relayed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = h.relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("parent request is published from the journal");
    match relayed {
        TxVerificationResult::UnknownParents {
            peer: relayed_peer,
            parents,
        } => {
            assert_eq!(relayed_peer, peer);
            assert_eq!(parents, HashSet::from([parent]));
        }
        other => panic!("unexpected relay result: {other:?}"),
    }

    h.cancel.cancel();
}

/// Administrative removal deletes an accepted root and every accepted
/// descendant. Coordinator consumers of any member of that closure must be
/// demoted before the pool mutation, not only consumers of the root.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_pool_closure_demotes_consumers_of_removed_descendants() {
    use super::harness::{WorkerSet, harness};
    use crate::component::entry::TxEntry;
    use crate::component::pipeline_coordinator::{CoordinatorLocation, RawStage};
    use crate::service::RemoveTxOutcome;
    use std::collections::HashSet;

    let h = harness(1).workers(WorkerSet::None).build();
    let root = build_tx(&h.out_points[0], 4_000);
    let child = build_tx(&OutPoint::new(root.hash(), 0), 3_000);
    let consumer = build_tx(&OutPoint::new(child.hash(), 0), 2_000);
    let root_id = root.proposal_short_id();
    let child_id = child.proposal_short_id();
    let consumer_hash = consumer.hash();
    {
        let mut pool = h.service.pool.tx_pool.write().await;
        for tx in [root.clone(), child.clone()] {
            pool.pool_map
                .add_entry(
                    TxEntry::dummy_resolve(tx, 0, Capacity::zero(), 100),
                    Status::Pending,
                )
                .unwrap();
        }
    }
    h.service
        .pipeline
        .runtime
        .admit_transaction(
            consumer,
            TxSource::Remote {
                cycles: 0,
                peer: 1.into(),
            },
            h.service.current_pipeline_epoch().unwrap(),
            RawStage::Resolve,
        )
        .unwrap();

    assert_eq!(
        h.service.remove_tx(root.hash()).await,
        RemoveTxOutcome::Removed
    );
    let pool = h.service.pool.tx_pool.read().await;
    assert!(pool.get_tx_from_pool(&root_id).is_none());
    assert!(pool.get_tx_from_pool(&child_id).is_none());
    drop(pool);
    assert_eq!(
        h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.view(&consumer_hash).unwrap().location),
        CoordinatorLocation::WaitingParents {
            missing: HashSet::from([child.hash()])
        }
    );

    h.cancel.cancel();
}

/// Freeing an accepted input is the linearization point for historical
/// conflict recovery. Administrative removal records durable transfer work
/// under the pool lock; maintenance then moves the candidate to the sole
/// executable coordinator owner exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_pool_entry_transfers_unblocked_conflict_cache_candidate_once() {
    use super::harness::{WorkerSet, harness};
    use crate::component::entry::TxEntry;
    use crate::service::RemoveTxOutcome;

    let h = harness(1).workers(WorkerSet::None).build();
    let blocker = build_tx(&h.out_points[0], 4_000);
    let candidate = build_tx(&h.out_points[0], 3_000);
    let candidate_hash = candidate.hash();
    {
        let mut pool = h.service.pool.tx_pool.write().await;
        pool.pool_map
            .add_entry(
                TxEntry::dummy_resolve(blocker.clone(), 0, Capacity::zero(), 100),
                Status::Pending,
            )
            .unwrap();
        pool.record_conflict(candidate.clone(), TxSource::Local);
    }

    assert_eq!(
        h.service.remove_tx(blocker.hash()).await,
        RemoveTxOutcome::Removed
    );
    {
        let pool = h.service.pool.tx_pool.read().await;
        assert!(pool.conflict_cache.contains_hash(&candidate_hash));
        assert_eq!(pool.conflict_recovery_len(), 0);
        assert_eq!(pool.conflict_discovery_len(), 1);
    }
    assert!(
        !h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&candidate_hash))
    );

    let progress = h.service.recover_conflict_cache_slice(1).await;
    assert!(!progress.capacity_blocked);
    assert!(
        h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&candidate_hash))
    );
    let pool = h.service.pool.tx_pool.read().await;
    assert!(!pool.conflict_cache.contains_hash(&candidate_hash));
    assert_eq!(pool.conflict_recovery_len(), 0);
    assert_eq!(pool.conflict_discovery_len(), 0);
    drop(pool);

    assert!(!h.service.recover_conflict_cache_slice(1).await.saturated);
    h.cancel.cancel();
}

/// A historical Local candidate can be scheduled just before the same raw
/// hash arrives from the higher-trust Proposal path. Recovery must consume the
/// stale cache owner without asking the coordinator to downgrade or replace
/// the Proposal witness; the old behavior escalated `SourceDowngrade` into a
/// service-wide fail-stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conflict_recovery_yields_to_existing_proposal_without_fail_stop() {
    use super::harness::{WorkerSet, harness};
    use crate::component::pipeline_coordinator::{CoordinatorSource, RawStage};

    let h = harness(1).workers(WorkerSet::None).build();
    let historical = build_tx(&h.out_points[0], 3_000);
    let hash = historical.hash();
    let proposal = historical
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"proposal-variant").pack()])
        .build();
    let proposal_witness = proposal.witness_hash();
    assert_eq!(proposal.hash(), hash);
    assert_ne!(proposal_witness, historical.witness_hash());
    {
        let mut pool = h.service.pool.tx_pool.write().await;
        pool.record_conflict(historical, TxSource::Local);
        assert_eq!(
            pool.schedule_conflict_candidates([hash.clone()].into_iter()),
            1
        );
    }
    let epoch = h.service.current_pipeline_epoch().expect("current epoch");
    assert!(
        h.service
            .pipeline
            .runtime
            .admit_transaction(proposal, TxSource::Proposal, epoch, RawStage::PreCheck)
            .expect("proposal admission")
            .0
    );

    let progress = h.service.recover_conflict_cache_slice(1).await;
    assert!(!progress.capacity_blocked);
    assert!(
        !h.service
            .pool
            .tx_pool
            .read()
            .await
            .conflict_cache
            .contains_hash(&hash),
        "the stronger coordinator owner consumes stale historical ownership"
    );
    h.service.pipeline.runtime.read(|coordinator| {
        assert_eq!(
            coordinator.view(&hash).expect("coordinator owner").source,
            CoordinatorSource::Proposal
        );
        assert_eq!(
            coordinator
                .raw_by_hash(&hash)
                .expect("proposal payload")
                .tx
                .witness_hash(),
            proposal_witness
        );
    });
    assert!(!h.service.pipeline.runtime.is_failed());
    assert!(!h.cancel.is_cancelled());
    h.cancel.cancel();
}

/// Pipeline clear is also an epoch barrier for cache-owned recovery work.
/// Historical conflict visibility remains, but an old scheduled transfer may
/// not recreate coordinator ownership after the clear returns.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clear_pipeline_cancels_conflict_recovery_schedule_without_deleting_history() {
    use super::harness::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let candidate = build_tx(&h.out_points[0], 4_000);
    let candidate_hash = candidate.hash();
    {
        let mut pool = h.service.pool.tx_pool.write().await;
        pool.record_conflict(candidate, TxSource::Local);
        assert_eq!(
            pool.schedule_conflict_candidates(std::iter::once(candidate_hash.clone())),
            1
        );
        assert_eq!(pool.conflict_recovery_len(), 1);
    }

    h.service.clear_pipeline().await;
    {
        let pool = h.service.pool.tx_pool.read().await;
        assert!(pool.conflict_cache.contains_hash(&candidate_hash));
        assert_eq!(pool.conflict_recovery_len(), 0);
    }
    assert!(
        !h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&candidate_hash))
    );
    let progress = h.service.recover_conflict_cache_slice(1).await;
    assert!(!progress.saturated && !progress.capacity_blocked);
    assert!(
        !h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&candidate_hash))
    );

    h.cancel.cancel();
}

/// ConflictCache owns complete transaction identities, while PoolMap and the
/// proposal protocol can host only one transaction per short ID. A colliding
/// accepted entry must therefore park—not delete—the historical candidate;
/// once the protocol slot is free, the same cache generation can transfer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conflict_recovery_retries_pool_short_id_collision_without_losing_history() {
    use super::harness::{WorkerSet, harness};
    use crate::component::entry::TxEntry;

    let h = harness(2).workers(WorkerSet::None).build();
    let mut accepted_hash = [0x42; 32];
    let mut cached_hash = accepted_hash;
    accepted_hash[31] = 1;
    cached_hash[31] = 2;
    let accepted = with_cached_hash(
        build_tx(&h.out_points[0], 4_000),
        ckb_types::packed::Byte32::new(accepted_hash),
    );
    let candidate = with_cached_hash(
        build_tx(&h.out_points[1], 3_000),
        ckb_types::packed::Byte32::new(cached_hash),
    );
    assert_eq!(accepted.proposal_short_id(), candidate.proposal_short_id());
    assert_ne!(accepted.hash(), candidate.hash());
    let accepted_tx_hash = accepted.hash();
    let candidate_hash = candidate.hash();
    let accepted_id = accepted.proposal_short_id();

    {
        let mut pool = h.service.pool.tx_pool.write().await;
        pool.pool_map
            .add_entry(
                TxEntry::dummy_resolve(accepted.clone(), 0, Capacity::zero(), 100),
                Status::Pending,
            )
            .unwrap();
        pool.record_conflict(candidate, TxSource::Local);
        assert_eq!(
            pool.schedule_conflict_candidates(std::iter::once(candidate_hash.clone())),
            1
        );
    }

    let blocked = h.service.recover_conflict_cache_slice(1).await;
    assert!(blocked.capacity_blocked);
    {
        let pool = h.service.pool.tx_pool.read().await;
        assert!(pool.conflict_cache.contains_hash(&candidate_hash));
        assert_eq!(pool.conflict_recovery_len(), 1);
    }
    assert!(
        !h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&candidate_hash))
    );

    h.service
        .pool
        .tx_pool
        .write()
        .await
        .pool_map
        .remove_entry(&accepted_id)
        .expect("colliding accepted entry remains present");

    let epoch = h.service.current_pipeline_epoch().unwrap();
    h.service
        .pipeline
        .runtime
        .admit_transaction_journaled(
            accepted,
            TxSource::Local,
            epoch,
            crate::component::pipeline_coordinator::RawStage::Resolve,
            |_| {},
        )
        .unwrap();
    let coordinator_blocked = h.service.recover_conflict_cache_slice(1).await;
    assert!(coordinator_blocked.capacity_blocked);
    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .conflict_cache
            .contains_hash(&candidate_hash)
    );
    h.service.pipeline.runtime.mutate_required(
        "test collision owner removal failed",
        |coordinator| {
            coordinator.force_terminalize(
                &accepted_tx_hash,
                crate::component::pipeline_coordinator::TerminalDisposition::Removed,
            )
        },
    );

    let recovered = h.service.recover_conflict_cache_slice(1).await;
    assert!(!recovered.capacity_blocked);
    assert!(
        h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&candidate_hash))
    );
    assert!(
        !h.service
            .pool
            .tx_pool
            .read()
            .await
            .conflict_cache
            .contains_hash(&candidate_hash)
    );

    h.cancel.cancel();
}

/// Failed detached recovery removes accepted descendants. Their independent
/// inputs are release events too: without scheduling ConflictCache discovery,
/// a valid historical competitor remains cache-owned forever even though its
/// blocker has disappeared.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_reorg_recovery_cascade_wakes_conflict_history() {
    use super::harness::{WorkerSet, harness};
    use crate::component::entry::TxEntry;

    let h = harness(2).workers(WorkerSet::None).build();
    let failed = build_tx(&h.out_points[0], 4_000);
    let failed_output = OutPoint::new(failed.hash(), 0);
    let independent_input = h.out_points[1].clone();
    let child = TransactionBuilder::default()
        .cell_dep(always_success_dep())
        .input(CellInput::new(failed_output, 0))
        .input(CellInput::new(independent_input.clone(), 0))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(3_000).unwrap())
                .lock(always_success_script())
                .build(),
        )
        .output_data(Bytes::default().pack())
        .build();
    let child_id = child.proposal_short_id();
    let competitor = build_tx(&independent_input, 3_500);
    let competitor_hash = competitor.hash();
    {
        let mut pool = h.service.pool.tx_pool.write().await;
        pool.pool_map
            .add_entry(
                TxEntry::dummy_resolve(child, 0, Capacity::zero(), 100),
                Status::Pending,
            )
            .unwrap();
        pool.record_conflict(competitor, TxSource::Local);
    }

    h.service.cascade_failed_reorg_recovery(&failed).await;
    {
        let pool = h.service.pool.tx_pool.read().await;
        assert!(pool.get_pool_entry(&child_id).is_none());
        assert!(pool.conflict_cache.contains_hash(&competitor_hash));
        assert_ne!(
            pool.conflict_discovery_len(),
            0,
            "the released independent input must become level-triggered work"
        );
    }

    let progress = h.service.recover_conflict_cache_slice(8).await;
    assert!(!progress.capacity_blocked);
    assert!(
        h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&competitor_hash))
    );
    assert!(
        !h.service
            .pool
            .tx_pool
            .read()
            .await
            .conflict_cache
            .contains_hash(&competitor_hash)
    );

    h.cancel.cancel();
}

/// A successful replacement removes an accepted parent without changing the
/// chain tip. Its already-resolved coordinator consumers must be demoted in
/// the same pool/coordinator commit transaction or they could commit using a
/// stale `ResolvedTransaction`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_rbf_commit_demotes_in_flight_consumers_of_removed_parent() {
    use super::harness::{WorkerSet, harness};
    use crate::component::pipeline_coordinator::{
        CoordinatorLocation, PayloadPhase, RawStage, VerifySchedule,
    };
    use crate::component::pipeline_runtime::resolved_charge_bytes;
    use crate::resolved_tx::ResolveJob;
    use std::collections::HashSet;

    let h = harness(1).rbf(true).workers(WorkerSet::None).build();
    let original = build_tx(&h.out_points[0], 4_000);
    h.service
        .process_tx(original.clone(), TxSource::Local)
        .await
        .expect("original enters the accepted pool");

    let consumer = build_tx(&OutPoint::new(original.hash(), 0), 3_000);
    let consumer_hash = consumer.hash();
    let epoch = h.service.current_pipeline_epoch().unwrap();
    h.service
        .pipeline
        .runtime
        .admit_transaction(
            consumer.clone(),
            TxSource::Remote {
                cycles: 0,
                peer: 1.into(),
            },
            epoch,
            RawStage::Resolve,
        )
        .unwrap();
    let lease = h
        .service
        .pipeline
        .runtime
        .checkout_raw(RawStage::Resolve)
        .unwrap();
    let resolved = match crate::resolve_mgr::resolve_job(
        &h.service,
        ResolveJob::new_at(
            consumer,
            TxSource::Remote {
                cycles: 0,
                peer: 1.into(),
            },
            epoch,
        ),
    )
    .await
    {
        crate::resolve_mgr::ResolveStageResult::Ready(resolved) => resolved,
        other => panic!("consumer should resolve against original: {other:?}"),
    };
    let charge = resolved_charge_bytes(&resolved).unwrap();
    h.service
        .pipeline
        .runtime
        .mutate(|coordinator| {
            coordinator.complete_raw(&lease, resolved, charge, VerifySchedule::default())
        })
        .unwrap();

    let replacement = build_tx(&h.out_points[0], 3_000);
    h.service
        .process_tx(replacement.clone(), TxSource::Local)
        .await
        .expect("higher-fee replacement commits");
    let pool = h.service.pool.tx_pool.read().await;
    assert!(
        pool.get_tx_from_pool(&original.proposal_short_id())
            .is_none()
    );
    assert!(
        pool.get_tx_from_pool(&replacement.proposal_short_id())
            .is_some()
    );
    drop(pool);
    let view = h
        .service
        .pipeline
        .runtime
        .read(|coordinator| coordinator.view(&consumer_hash).unwrap());
    assert_eq!(view.phase, PayloadPhase::Raw);
    assert_eq!(
        view.location,
        CoordinatorLocation::WaitingParents {
            missing: HashSet::from([original.hash()])
        }
    );

    h.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_preserves_order_for_dependent_txs() {
    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline(1);
    let issue_out_point = &issue_out_points[0];

    // tx_a creates an output; tx_b spends it.
    let tx_a = build_tx(issue_out_point, 4_000);
    let tx_a_output = OutPoint::new(tx_a.hash(), 0);
    let tx_b = build_tx(&tx_a_output, 3_000);

    // Submit B first, then A. Because resolve is ordered, A should be resolved
    // and submitted before B is re-resolved against the pool.
    let tx_a_cycles = measured_cycles(&service, tx_a.clone()).await;
    // tx_b spends tx_a's output, so it cannot be measured until tx_a is in the
    // pool.  For the always-success script the verification cost is identical
    // for both transactions, so we reuse tx_a's cycle count for tx_b.
    service
        .submit_remote_tx(
            tx_b.clone(),
            TxSource::Remote {
                cycles: tx_a_cycles,
                peer: 1.into(),
            },
        )
        .await
        .unwrap();

    service
        .submit_remote_tx(
            tx_a.clone(),
            TxSource::Remote {
                cycles: tx_a_cycles,
                peer: 1.into(),
            },
        )
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("pipeline should process dependent txs in time");

    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(pending, 2);

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_rejects_conflicting_double_spend() {
    // Two remote txs spend the same chain output concurrently.
    // The pool must accept exactly one and reject the other; it must never
    // end up with both or panic.
    let tx_count = 2;
    let (service, _relay, signal, _store, issue_out_points) =
        service_with_pipeline_workers(tx_count, 4);

    let shared_input = issue_out_points.first().expect("at least one issue out");
    let tx_a = build_tx(shared_input, 4_000);
    let id_a = tx_a.proposal_short_id();
    // tx_b spends the same input but pays to a different output so it has a
    // different hash.
    let tx_b = TransactionBuilder::default()
        .cell_dep(always_success_dep())
        .input(CellInput::new(shared_input.clone(), 0))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(4_990).unwrap())
                .lock(Script::default())
                .build(),
        )
        .output_data(Bytes::default().pack())
        .build();
    let id_b = tx_b.proposal_short_id();

    let cycles_a = measured_cycles(&service, tx_a.clone()).await;
    let cycles_b = measured_cycles(&service, tx_b.clone()).await;

    let service_a = service.clone();
    let service_b = service.clone();
    let handle_a = tokio::spawn(async move {
        service_a
            .submit_remote_tx(
                tx_a,
                TxSource::Remote {
                    cycles: cycles_a,
                    peer: 1.into(),
                },
            )
            .await
    });
    let handle_b = tokio::spawn(async move {
        service_b
            .submit_remote_tx(
                tx_b,
                TxSource::Remote {
                    cycles: cycles_b,
                    peer: 1.into(),
                },
            )
            .await
    });

    let (res_a, res_b) = tokio::join!(handle_a, handle_b);
    let _ = res_a.expect("task a should not panic");
    let _ = res_b.expect("task b should not panic");

    // Wait for the pipeline to drain. Both txs should leave the ordered/verify
    // queues and exactly one must land in the pending pool.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (pending, pipeline_len) = {
                let pool = service.pool.tx_pool.read().await;
                let pipeline_len = service
                    .pipeline
                    .runtime
                    .read(|coordinator| coordinator.len());
                (pool.pool_map.pending_size(), pipeline_len)
            };
            if pending == 1 && pipeline_len == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("pipeline should settle with exactly one double-spend tx accepted");

    let pool = service.pool.tx_pool.read().await;
    let a_in_pool = pool.get_tx_from_pool(&id_a).is_some();
    let b_in_pool = pool.get_tx_from_pool(&id_b).is_some();
    assert!(
        a_in_pool ^ b_in_pool,
        "exactly one of the double-spend txs must be in the pool, got a={a_in_pool} b={b_in_pool}"
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_preserves_cell_dep_before_in_flight_consumer() {
    // tx_a spends an on-chain cell X. tx_b spends a different cell but uses X as
    // a cell dep. Both can coexist when tx_b commits first: the pool records tx_b
    // as tx_a's ancestor so block assembly uses X as a dep before consuming it.
    // If tx_a commits first, tx_b is correctly rejected as Dead. The concurrent
    // pipeline may reach either valid state, but never the invalid reverse order.
    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline_workers(2, 4);
    let input_a = &issue_out_points[0];
    let input_b = &issue_out_points[1];

    let tx_a = build_tx(input_a, 4_000);
    let tx_b = TransactionBuilder::default()
        .cell_dep(always_success_dep())
        .cell_dep(CellDep::new_builder().out_point(input_a.clone()).build())
        .input(CellInput::new(input_b.clone(), 0))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(4_000).unwrap())
                .lock(Script::default())
                .build(),
        )
        .output_data(Bytes::default().pack())
        .build();
    let id_a = tx_a.proposal_short_id();
    let id_b = tx_b.proposal_short_id();

    let cycles_a = measured_cycles(&service, tx_a.clone()).await;
    let cycles_b = measured_cycles(&service, tx_b.clone()).await;

    // Submit tx_a first and wait until it is actually in flight (either in the
    // verify queue or already accepted).  Only then submit tx_b so that the
    // cell-dep-on-in-flight-input path is exercised deterministically.
    service
        .submit_remote_tx(
            tx_a.clone(),
            TxSource::Remote {
                cycles: cycles_a,
                peer: 1.into(),
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (pending, in_pipeline) = {
                let pool = service.pool.tx_pool.read().await;
                let in_pipeline = service
                    .pipeline
                    .runtime
                    .read(|coordinator| coordinator.contains_hash(&tx_a.hash()));
                (pool.pool_map.pending_size(), in_pipeline)
            };
            if pending == 1 || in_pipeline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("tx_a should enter the pipeline");

    service
        .submit_remote_tx(
            tx_b.clone(),
            TxSource::Remote {
                cycles: cycles_b,
                peer: 2.into(),
            },
        )
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (pending, pipeline_len) = {
                let pool = service.pool.tx_pool.read().await;
                let pipeline_len = service
                    .pipeline
                    .runtime
                    .read(|coordinator| coordinator.len());
                (pool.pool_map.pending_size(), pipeline_len)
            };
            if pending >= 1 && pipeline_len == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("pipeline should settle to a valid dep-before-consumer state");

    let pool = service.pool.tx_pool.read().await;
    assert!(
        pool.get_tx_from_pool(&id_a).is_some(),
        "tx_a should be accepted"
    );
    if pool.get_tx_from_pool(&id_b).is_some() {
        assert!(
            pool.pool_map.calc_ancestors(&id_a).contains(&id_b),
            "when both transactions are accepted, the dep user must precede the consumer"
        );
    }

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_allows_same_cell_as_input_and_cell_dep() {
    // CKB permits a transaction to reference the same out-point both as an
    // input and as a cell dep. The pipeline must not reject such a tx with
    // OutPointError::Dead.
    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline_workers(2, 4);
    let input_a = &issue_out_points[0];

    let tx_a = build_tx(input_a, 4_000);
    let output_a = OutPoint::new(tx_a.hash(), 0);

    // tx_b consumes tx_a's output and also references it as a cell dep.
    let tx_b = TransactionBuilder::default()
        .cell_dep(always_success_dep())
        .cell_dep(CellDep::new_builder().out_point(output_a.clone()).build())
        .input(CellInput::new(output_a.clone(), 0))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(3_000).unwrap())
                .lock(Script::default())
                .build(),
        )
        .output_data(Bytes::default().pack())
        .build();
    let id_a = tx_a.proposal_short_id();
    let id_b = tx_b.proposal_short_id();

    // Submit tx_a first and wait until it is accepted.
    service
        .process_tx(tx_a.clone(), TxSource::Local)
        .await
        .expect("tx_a should be accepted");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = {
                let pool = service.pool.tx_pool.read().await;
                pool.pool_map.pending_size()
            };
            if pending == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("tx_a should settle");

    // Now tx_b's input and cell dep point to the same in-pool out-point.
    service
        .process_tx(tx_b.clone(), TxSource::Local)
        .await
        .expect("tx_b should be accepted even though its cell dep is also its input");

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = {
                let pool = service.pool.tx_pool.read().await;
                pool.pool_map.pending_size()
            };
            if pending == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("tx_b should settle");

    let pool = service.pool.tx_pool.read().await;
    assert!(
        pool.get_tx_from_pool(&id_a).is_some(),
        "tx_a should be accepted"
    );
    assert!(
        pool.get_tx_from_pool(&id_b).is_some(),
        "tx_b should be accepted even though its cell dep is also its input"
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_processes_independent_secp_remote_txs() {
    // Verify that the concurrent pre-resolver handles real secp256k1 1-in-1-out
    // transactions (not always-success scripts) correctly.
    let tx_count = 10;
    let (service, _relay, signal, _store, issue_out_points, cell_deps) =
        secp_service_with_pipeline_workers(tx_count, 4);

    let txs: Vec<_> = issue_out_points
        .iter()
        .map(|out_point| build_secp_tx(out_point, &cell_deps, SECP_ISSUE_CAPACITY - SECP_FEE))
        .collect();

    let mut cycles = Vec::with_capacity(txs.len());
    for tx in &txs {
        cycles.push(verify_cycles(&service, tx.clone()).await);
    }

    for (tx, cycles) in txs.iter().zip(&cycles) {
        service
            .submit_remote_tx(
                tx.clone(),
                TxSource::Remote {
                    cycles: *cycles,
                    peer: 1.into(),
                },
            )
            .await
            .expect("enqueue secp remote tx should succeed");
    }

    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == txs.len() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("pipeline should process all independent secp txs in time");

    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(pending, txs.len());

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_preserves_order_for_dependent_secp_txs() {
    // Realistic dependent chain: parent is a secp256k1 1-in-1-out tx, child spends
    // its output.  Submitting child before parent must still end with both in the
    // pool because the ordered resolver/orphan recovery preserves order.
    let (service, _relay, signal, _store, issue_out_points, cell_deps) =
        secp_service_with_pipeline_workers(1, 4);
    let issue_out_point = &issue_out_points[0];

    let parent = build_secp_tx(issue_out_point, &cell_deps, SECP_ISSUE_CAPACITY - SECP_FEE);
    let parent_output = OutPoint::new(parent.hash(), 0);
    let child = build_secp_tx(
        &parent_output,
        &cell_deps,
        SECP_ISSUE_CAPACITY - 2 * SECP_FEE,
    );

    // Put parent into the pool temporarily so we can measure the child's exact
    // verification cycles, then remove it so the child must go through orphan
    // recovery when submitted before the parent.
    let parent_cycles = submit_local_tx(&service, parent.clone()).await;
    let child_cycles = verify_cycles(&service, child.clone()).await;
    service.remove_tx(parent.hash()).await;

    // Submit child first; it cannot resolve yet because the parent output is not
    // in the chain nor in any queue.
    service
        .submit_remote_tx(
            child.clone(),
            TxSource::Remote {
                cycles: child_cycles,
                peer: 1.into(),
            },
        )
        .await
        .expect("enqueue child secp tx should succeed");

    service
        .submit_remote_tx(
            parent.clone(),
            TxSource::Remote {
                cycles: parent_cycles,
                peer: 1.into(),
            },
        )
        .await
        .expect("enqueue parent secp tx should succeed");

    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("pipeline should process dependent secp txs in order");

    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(pending, 2);

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// An attached block can commit a remote transaction before its coordinator
/// worker reaches verification. Removing that sole lifecycle owner must also
/// publish the ingress success in the same reorg effect transaction.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attached_commit_settles_pre_pool_remote_ingress() {
    use super::harness::{WorkerSet, harness};
    use crate::service::TxVerificationResult;
    use std::collections::{HashSet, VecDeque};

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    let peer = ckb_network::PeerIndex::from(77);
    assert!(
        h.service
            .submit_remote_tx(tx.clone(), TxSource::Remote { cycles: 0, peer },)
            .await
            .unwrap()
    );
    assert!(
        h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&hash))
    );

    let attached = BlockBuilder::default()
        .transaction(TransactionBuilder::default().build())
        .transaction(tx)
        .build();
    let snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();
    h.service
        .update_tx_pool_for_reorg(
            VecDeque::new(),
            VecDeque::from([attached]),
            HashSet::new(),
            snapshot,
        )
        .await
        .unwrap();
    assert!(
        !h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&hash))
    );

    let relayed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = h.relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("chain commit must release the remote ingress filter");
    assert!(matches!(
        relayed,
        TxVerificationResult::Ok {
            original_peer: Some(relayed_peer),
            tx_hash,
        } if relayed_peer == peer && tx_hash == hash
    ));
    h.cancel.cancel();
}

/// Test that `update_tx_pool_for_reorg` correctly routes retained (detached)
/// transactions through the pipeline entry point rather than blocking the
/// write lock with inline verification.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_reorg_routes_retained_txs_through_classify() {
    use std::collections::{HashSet, VecDeque};

    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline(3);

    // Submit 3 independent txs and wait for all to be pending.
    let txs: Vec<_> = issue_out_points
        .iter()
        .map(|out_point| build_tx(out_point, 4_000))
        .collect();

    for tx in &txs {
        let cycles = measured_cycles(&service, tx.clone()).await;
        service
            .submit_remote_tx(
                tx.clone(),
                TxSource::Remote {
                    cycles,
                    peer: 1.into(),
                },
            )
            .await
            .expect("enqueue remote tx should succeed");
    }

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == txs.len() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("all txs should be pending before reorg");

    // Build a "detached" block that contains the first 2 txs.
    // This simulates a block being orphaned during a reorg.
    let detached_block = BlockBuilder::default()
        .number(1)
        .parent_hash(service.pool.tx_pool.read().await.snapshot.tip_hash())
        .epoch(EpochNumberWithFraction::new(0, 0, 1).full_value())
        .transaction(
            // cellbase (placeholder — skip(1) in reorg handler skips this)
            TransactionBuilder::default()
                .input(CellInput::new(OutPoint::null(), 0))
                .output(
                    CellOutput::new_builder()
                        .capacity(Capacity::bytes(1_000).unwrap())
                        .build(),
                )
                .output_data(Bytes::default().pack())
                .build(),
        )
        .transaction(txs[0].clone())
        .transaction(txs[1].clone())
        .build();

    let detached_blocks: VecDeque<BlockView> = [detached_block].into();
    let attached_blocks: VecDeque<BlockView> = VecDeque::new();
    let detached_proposal_id: HashSet<ProposalShortId> = HashSet::new();
    let snapshot = service.pool.tx_pool.read().await.cloned_snapshot();

    // Trigger the reorg. This should call classify_and_enqueue_tx for each
    // retained tx after releasing the write lock. The calls will fail with
    // "already in pool" errors (expected), but the critical thing is:
    // - No panic
    // - Pool remains consistent
    // - classify_and_enqueue_tx is exercised
    service
        .update_tx_pool_for_reorg(
            detached_blocks,
            attached_blocks,
            detached_proposal_id,
            snapshot,
        )
        .await
        .unwrap();

    // Give the pipeline a moment to process any classify_and_enqueue_tx calls.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Pool should still contain all 3 txs (reorg didn't remove anything
    // since attached was empty and the txs were in pending, not committed).
    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(
        pending, 3,
        "pool should still have all 3 txs after reorg with empty attached"
    );

    assert_eq!(
        service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.len()),
        0,
        "coordinator should be empty after duplicate reorg recovery"
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

// ---------------------------------------------------------------------------
// Additional helpers for specialized test configurations
// ---------------------------------------------------------------------------

/// Same as `service_with_pipeline` but enables RBF by setting `min_rbf_rate`
/// above `min_fee_rate`.
pub(crate) fn service_with_rbf(
    issue_outputs: usize,
) -> (
    TxPoolService,
    ckb_channel::Receiver<crate::service::TxVerificationResult>,
    CancellationToken,
    MockStore,
    Vec<OutPoint>,
) {
    let h = super::harness::harness(issue_outputs).rbf(true).build();
    (h.service, h.relay_rx, h.cancel, h.store, h.out_points)
}

/// Same as `service_with_rbf` but with a custom `max_tx_pool_size`. Used to
/// force `limit_size` to reject a replacement after the original transaction
/// has already been removed by RBF.
#[allow(clippy::type_complexity)]
fn service_with_rbf_and_max_size(
    issue_outputs: usize,
    max_tx_pool_size: usize,
) -> (
    TxPoolService,
    ckb_channel::Receiver<crate::service::TxVerificationResult>,
    CancellationToken,
    MockStore,
    Vec<OutPoint>,
) {
    let h = super::harness::harness(issue_outputs)
        .rbf(true)
        .max_tx_pool_size(max_tx_pool_size)
        .build();
    (h.service, h.relay_rx, h.cancel, h.store, h.out_points)
}

/// Same as `secp_service_with_pipeline_workers` but also returns
/// `watch::Sender<ChunkCommand>` so tests can send Suspend/Resume signals.
#[allow(clippy::type_complexity)]
fn secp_service_with_pipeline_workers_and_chunk(
    issue_outputs: usize,
    max_workers: usize,
) -> (
    TxPoolService,
    ckb_channel::Receiver<crate::service::TxVerificationResult>,
    CancellationToken,
    MockStore,
    Vec<OutPoint>,
    Vec<CellDep>,
    watch::Sender<ChunkCommand>,
) {
    let h = super::harness::harness(issue_outputs)
        .secp(true)
        .max_workers(max_workers)
        .with_chunk_sender(true)
        .build();
    (
        h.service,
        h.relay_rx,
        h.cancel,
        h.store,
        h.out_points,
        h.cell_deps.expect("secp harness provides cell deps"),
        h.chunk_tx.expect("chunk sender requested"),
    )
}

// ---------------------------------------------------------------------------
// Integration tests: dedup, worker cap, backpressure, pause/resume, RBF
// ---------------------------------------------------------------------------

/// Submitting the same transaction twice must not duplicate it in the pool.
/// The second submission should be silently deduplicated — the pool must
/// contain exactly one copy.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_dedup_double_submission() {
    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline(1);

    let tx = build_tx(&issue_out_points[0], 4_000);
    let id = tx.proposal_short_id();
    let cycles = measured_cycles(&service, tx.clone()).await;

    // First submission.
    service
        .submit_remote_tx(
            tx.clone(),
            TxSource::Remote {
                cycles,
                peer: 1.into(),
            },
        )
        .await
        .expect("first submission should succeed");

    // Wait for the tx to reach the pending pool.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("tx should reach pending");

    // Second submission of the same tx.
    // The coordinator and accepted pool jointly deduplicate the submission;
    // pool_map.add_entry also returns `inserted == false` for an existing short ID.
    // Either way, the pool must still have exactly 1 tx.
    let second_result = service
        .submit_remote_tx(
            tx.clone(),
            TxSource::Remote {
                cycles,
                peer: 1.into(),
            },
        )
        .await;
    // The result may be Ok (silent dedup in pool_map) or Err(Duplicated).
    // Both are correct behavior — what matters is the pool state.
    assert!(matches!(
        second_result,
        Ok(_) | Err(crate::error::Reject::Duplicated(_))
    ));

    // Brief wait for any in-flight processing.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(
        pending, 1,
        "pool must have exactly 1 tx after duplicate submission"
    );

    // Verify the specific tx is still in the pool.
    let pool = service.pool.tx_pool.read().await;
    assert!(
        pool.get_tx_from_pool(&id).is_some(),
        "original tx should still be in pool"
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// Creating a service with `max_tx_verify_workers` far exceeding the machine's
/// available parallelism should not panic or cause resource issues. The
/// PreCheckQueue worker cap (`min(max_workers, available_parallelism)`) should
/// keep the actual worker count reasonable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_high_pre_check_worker_cap() {
    let (service, _relay, signal, _store, issue_out_points) =
        service_with_pipeline_workers(5, 1000);

    let txs: Vec<_> = issue_out_points
        .iter()
        .map(|out_point| build_tx(out_point, 4_000))
        .collect();

    for tx in &txs {
        let cycles = measured_cycles(&service, tx.clone()).await;
        service
            .submit_remote_tx(
                tx.clone(),
                TxSource::Remote {
                    cycles,
                    peer: 1.into(),
                },
            )
            .await
            .expect("enqueue remote tx should succeed");
    }

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == txs.len() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("pipeline should process all txs even with high worker cap");

    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(pending, txs.len());

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// With `max_workers = 1` (semaphore capacity = 2), flooding the pipeline
/// with many concurrent submissions must not lose any transactions. The
/// semaphore provides backpressure: when all permits are consumed, the actor
/// loop blocks on `acquire_owned()`, but no messages are dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_semaphore_backpressure() {
    let tx_count = 10;
    let (service, _relay, signal, _store, issue_out_points) =
        service_with_pipeline_workers(tx_count, 1);

    let txs: Vec<_> = issue_out_points
        .iter()
        .map(|out_point| build_tx(out_point, 4_000))
        .collect();

    let mut cycles_vec = Vec::with_capacity(txs.len());
    for tx in &txs {
        cycles_vec.push(measured_cycles(&service, tx.clone()).await);
    }

    // Submit all txs concurrently. With semaphore cap = 2, at most 2
    // process() calls run simultaneously. All 10 must still complete.
    let mut handles = Vec::new();
    for (tx, cycles) in txs.iter().zip(&cycles_vec) {
        let svc = service.clone();
        let tx = tx.clone();
        let cycles = *cycles;
        handles.push(tokio::spawn(async move {
            svc.submit_remote_tx(
                tx,
                TxSource::Remote {
                    cycles,
                    peer: 1.into(),
                },
            )
            .await
            .expect("submit under backpressure should succeed");
        }));
    }
    for h in handles {
        h.await.expect("submit task should not panic");
    }

    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == tx_count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("all txs should reach pending despite semaphore backpressure");

    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(
        pending, tx_count,
        "semaphore backpressure must not lose transactions"
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// The `ChunkCommand` watch signal propagates from `TxPoolController` through
/// `VerifyMgr` and `OrderedResolver`. This test verifies the signal path
/// end-to-end using real secp256k1 transactions:
///
/// 1. Submit secp txs with 1 worker (slow, sequential verification).
/// 2. Send `ChunkCommand::Suspend` — VerifyMgr stops picking up new work.
/// 3. Send `ChunkCommand::Resume` — verification resumes.
/// 4. All txs must eventually reach pending.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_chunk_command_pause_resume() {
    let (service, _relay, signal, _store, issue_out_points, cell_deps, chunk_tx) =
        secp_service_with_pipeline_workers_and_chunk(4, 1);

    let txs: Vec<_> = issue_out_points
        .iter()
        .map(|out_point| build_secp_tx(out_point, &cell_deps, SECP_ISSUE_CAPACITY - SECP_FEE))
        .collect();

    let mut cycles_vec = Vec::with_capacity(txs.len());
    for tx in &txs {
        cycles_vec.push(verify_cycles(&service, tx.clone()).await);
    }

    for (tx, cycles) in txs.iter().zip(&cycles_vec) {
        service
            .submit_remote_tx(
                tx.clone(),
                TxSource::Remote {
                    cycles: *cycles,
                    peer: 1.into(),
                },
            )
            .await
            .expect("enqueue secp tx should succeed");
    }

    // Brief yield to let the first tx start verifying.
    tokio::time::sleep(Duration::from_millis(5)).await;

    // Suspend — VerifyMgr stops picking up new VerifyQueue items.
    chunk_tx
        .send(ChunkCommand::Suspend)
        .expect("send suspend signal");

    // Wait briefly while suspended. In-flight verification continues, but
    // no new items are dequeued from VerifyQueue.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let pending_while_suspended = service.pool.tx_pool.read().await.pool_map.pending_size();

    // Resume — remaining txs should now drain through verification.
    chunk_tx
        .send(ChunkCommand::Resume)
        .expect("send resume signal");

    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == txs.len() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("all txs should reach pending after resume");

    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(pending, txs.len());

    // With 1 worker and suspend, some txs should have been delayed.
    assert!(
        pending_while_suspended < txs.len(),
        "suspend should have delayed some txs (got {pending_while_suspended}/{})",
        txs.len()
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// When RBF is enabled (`min_rbf_rate > min_fee_rate`), submitting a
/// higher-fee transaction that spends the same input as an existing
/// lower-fee transaction should:
///
/// 1. Remove the lower-fee tx from the pool (via `process_rbf`).
/// 2. Insert the higher-fee tx.
/// 3. Exercise the authoritative conflict-cache bookkeeping for the displaced
///    transaction.
///
/// This tests the full RBF → pool state transition path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_rbf_displaces_lower_fee_tx() {
    let (service, _relay, signal, _store, issue_out_points) = service_with_rbf(1);

    let shared_input = &issue_out_points[0];

    // tx_a: lower fee (input 5000 CKB → output 4998 CKB, fee = 2 CKB).
    let tx_a = build_tx(shared_input, 4_998);
    let id_a = tx_a.proposal_short_id();

    // tx_b: higher fee, same input (output 4990 CKB, fee = 10 CKB).
    let tx_b = build_tx(shared_input, 4_990);
    let id_b = tx_b.proposal_short_id();

    assert_ne!(id_a, id_b, "txs must have different proposal_short_ids");

    let cycles_a = measured_cycles(&service, tx_a.clone()).await;
    let cycles_b = measured_cycles(&service, tx_b.clone()).await;

    // Submit tx_a and wait for it to reach pending.
    service
        .submit_remote_tx(
            tx_a.clone(),
            TxSource::Remote {
                cycles: cycles_a,
                peer: 1.into(),
            },
        )
        .await
        .expect("tx_a should be accepted");

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("tx_a should reach pending");

    {
        let pool = service.pool.tx_pool.read().await;
        assert!(
            pool.get_tx_from_pool(&id_a).is_some(),
            "tx_a should be in pool before replacement"
        );
    }

    // Submit tx_b — triggers RBF, displacing tx_a.
    service
        .submit_remote_tx(
            tx_b.clone(),
            TxSource::Remote {
                cycles: cycles_b,
                peer: 1.into(),
            },
        )
        .await
        .expect("tx_b (RBF replacement) should be accepted");

    // Wait for RBF to complete: tx_b must appear in the pool, which can only
    // happen after tx_a is removed (they conflict on the same input).
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (b_in_pool, pipeline_len) = {
                let pool = service.pool.tx_pool.read().await;
                (
                    pool.get_tx_from_pool(&id_b).is_some(),
                    service
                        .pipeline
                        .runtime
                        .read(|coordinator| coordinator.len()),
                )
            };
            if b_in_pool && pipeline_len == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("RBF should complete: tx_a displaced, tx_b in pool");

    let pool = service.pool.tx_pool.read().await;
    assert!(
        pool.get_tx_from_pool(&id_a).is_none(),
        "tx_a should be removed after RBF"
    );
    assert!(
        pool.get_tx_from_pool(&id_b).is_some(),
        "tx_b (higher fee) should be in pool after RBF"
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// When an RBF replacement is rejected by the pool (e.g. it no longer fits
/// after the old tx is removed), the original conflicted transaction must be
/// recovered rather than silently dropped. This prevents a remote peer from
/// evicting an in-pool tx by submitting a replacement that passes RBF checks
/// but fails insertion.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_rbf_rejected_replacement_recovers_original_tx() {
    // Pool size just large enough for the small original tx but not for the
    // large replacement.
    let (service, _relay, signal, _store, issue_out_points) =
        service_with_rbf_and_max_size(1, 1_500);

    let shared_input = &issue_out_points[0];

    // tx_a: small tx in the pool.
    let tx_a = build_tx(shared_input, 4_998);
    let id_a = tx_a.proposal_short_id();

    // tx_b: higher fee, same input, but with a large output_data so its
    // serialized size exceeds the tiny pool limit after tx_a is removed.
    let tx_b = TransactionBuilder::default()
        .cell_dep(always_success_dep())
        .input(CellInput::new(shared_input.clone(), 0))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(4_000).unwrap())
                .lock(always_success_script())
                .build(),
        )
        .output_data(Bytes::from(vec![0u8; 2_000]).pack())
        .build();
    let id_b = tx_b.proposal_short_id();

    assert_ne!(id_a, id_b, "txs must have different proposal_short_ids");

    let cycles_a = measured_cycles(&service, tx_a.clone()).await;
    let cycles_b = measured_cycles(&service, tx_b.clone()).await;

    // Submit tx_a and wait for it to reach pending.
    service
        .submit_remote_tx(
            tx_a.clone(),
            TxSource::Remote {
                cycles: cycles_a,
                peer: 1.into(),
            },
        )
        .await
        .expect("tx_a should be accepted");

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("tx_a should reach pending");

    // Submit tx_b. This merely enqueues the tx and returns Ok; actual
    // success/failure is determined by inspecting the final pool state.
    let _ = service
        .submit_remote_tx(
            tx_b.clone(),
            TxSource::Remote {
                cycles: cycles_b,
                peer: 1.into(),
            },
        )
        .await
        .unwrap();

    // Failed replacement rollback restores tx_a synchronously under the pool
    // write guard. The observable outcome is tx_a back in the pool.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            {
                let pool = service.pool.tx_pool.read().await;
                if pool.get_tx_from_pool(&id_a).is_some() {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("tx_a should be recovered after the rejected replacement");

    // tx_b passes RBF checks, removes tx_a, but is then rejected by
    // `limit_size` because the pool is too small. tx_a must be recovered from
    // the waiting room rather than left out of the mempool.
    let pool = service.pool.tx_pool.read().await;
    assert!(
        pool.get_tx_from_pool(&id_b).is_none(),
        "tx_b should be rejected because it exceeds the tiny pool size"
    );
    assert!(
        pool.get_tx_from_pool(&id_a).is_some(),
        "tx_a should be recovered after the rejected replacement"
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// Bug #45: a management clear is authoritative state replacement, not a
/// best-effort incremental update. The reset snapshot must survive a saturated
/// wake channel, blank the current template immediately, and notify external
/// miners without waiting for the periodic assembler interval.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clear_pool_resets_template_and_notifies_miner_immediately() {
    use super::harness::{WorkerSet, harness};
    use crate::block_assembler::BlockAssembler;
    use crate::service::BlockAssemblerMessage;
    use ckb_app_config::BlockAssemblerConfig;
    use ckb_jsonrpc_types::ScriptHashType;
    use std::sync::atomic::Ordering;

    let mut h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], ISSUE_OUTPUT_CAPACITY as usize - 1);
    h.service
        .process_tx(tx, TxSource::Local)
        .await
        .expect("seed one pending transaction");

    let snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();
    let assembler = BlockAssembler::new(
        BlockAssemblerConfig {
            code_hash: h256!("0x0"),
            args: Default::default(),
            hash_type: ScriptHashType::Data,
            message: Default::default(),
            use_binary_version_as_message_prefix: true,
            binary_version: "TEST".to_string(),
            update_interval_millis: 60_000,
            notify: vec![],
            notify_scripts: vec![],
            notify_timeout_millis: 800,
            notify_auth_token: None,
        },
        Arc::clone(&snapshot),
    )
    .unwrap();
    assembler.update_proposals(&h.service.pool.tx_pool).await;
    assert_eq!(assembler.get_current().await.proposals.len(), 1);
    let notify_count = Arc::clone(&assembler.notify_count);
    h.service.block_assembler = Some(assembler);

    // Occupy the one-slot wake channel before the clear. Reset authority must
    // live in the journal, not in the channel payload that now cannot enqueue.
    h.service
        .journal_block_assembler_message(BlockAssemblerMessage::Pending);

    h.service.clear_pool(Arc::clone(&snapshot)).await;
    let message = tokio::time::timeout(Duration::from_secs(1), h.block_assembler_rx.recv())
        .await
        .expect("clear_pool must not wait for the periodic interval")
        .expect("an existing wake must remain available");
    assert_eq!(message, BlockAssemblerMessage::Pending);

    // The production consumer drains Reset before every received wake.
    crate::block_assembler::process(h.service.clone(), &BlockAssemblerMessage::Reset).await;
    let current = h
        .service
        .block_assembler
        .as_ref()
        .expect("assembler installed")
        .get_current()
        .await;
    assert!(current.proposals.is_empty());
    assert!(current.transactions.is_empty());
    assert_eq!(notify_count.load(Ordering::SeqCst), 1);
    assert_eq!(h.service.pool.tx_pool.read().await.pool_map.size(), 0);
}

/// A template rebuild happens without holding the reset journal lock. If a
/// newer authoritative reset arrives while the older snapshot is being
/// rebuilt, acknowledging the older work must not erase the newer request —
/// even when both requests carry the exact same snapshot Arc.
#[tokio::test]
async fn stale_block_assembler_reset_ack_preserves_newer_generation() {
    use super::harness::harness;

    let h = harness(1).build();
    let snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();

    h.service
        .relay
        .mark_block_assembler_reset(Arc::clone(&snapshot));
    let (loaded_generation, loaded_snapshot) = h
        .service
        .relay
        .load_block_assembler_reset()
        .expect("older reset is journaled");
    assert!(Arc::ptr_eq(&loaded_snapshot, &snapshot));
    h.service
        .relay
        .mark_block_assembler_reset(Arc::clone(&snapshot));

    h.service
        .relay
        .complete_block_assembler_reset(loaded_generation);
    let (new_generation, still_pending) = h
        .service
        .relay
        .load_block_assembler_reset()
        .expect("stale acknowledgement must preserve the newer reset");
    assert!(new_generation > loaded_generation);
    assert!(Arc::ptr_eq(&still_pending, &snapshot));

    h.service
        .relay
        .complete_block_assembler_reset(new_generation);
    assert!(h.service.relay.load_block_assembler_reset().is_none());
}

/// A partial template update that observes the pool on a newer tip must not
/// consume its dirty generation. Once assembler and pool snapshots converge,
/// the same journal item is retried and conditionally acknowledged.
#[tokio::test]
async fn rejected_duplicate_uncle_does_not_retrigger_template_work() {
    use super::harness::{WorkerSet, harness};
    use crate::block_assembler::BlockAssembler;
    use crate::service::BlockAssemblerMessage;
    use ckb_app_config::BlockAssemblerConfig;
    use ckb_jsonrpc_types::ScriptHashType;

    let mut h = harness(1).workers(WorkerSet::None).build();
    let snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();
    h.service.block_assembler = Some(
        BlockAssembler::new(
            BlockAssemblerConfig {
                code_hash: h256!("0x0"),
                args: Default::default(),
                hash_type: ScriptHashType::Data,
                message: Default::default(),
                use_binary_version_as_message_prefix: true,
                binary_version: "TEST".to_string(),
                update_interval_millis: 60_000,
                notify: vec![],
                notify_scripts: vec![],
                notify_timeout_millis: 800,
                notify_auth_token: None,
            },
            Arc::clone(&snapshot),
        )
        .unwrap(),
    );
    let uncle = BlockBuilder::default()
        .parent_hash(snapshot.tip_hash())
        .number(snapshot.tip_number() + 1)
        .epoch(EpochNumberWithFraction::new(0, 0, 1))
        .build()
        .as_uncle();

    h.service.receive_candidate_uncle(uncle.clone()).await;
    let dirty = h.service.relay.load_block_assembler_dirty();
    let (_, generation) = dirty
        .iter()
        .find(|(message, _)| *message == BlockAssemblerMessage::Uncle)
        .expect("first candidate marks uncle work");
    h.service
        .relay
        .complete_block_assembler_dirty(&BlockAssemblerMessage::Uncle, *generation);
    assert!(h.service.relay.load_block_assembler_dirty().is_empty());

    h.service.receive_candidate_uncle(uncle).await;
    assert!(
        h.service.relay.load_block_assembler_dirty().is_empty(),
        "a rejected duplicate cannot amplify into repeated template rebuilds"
    );
    h.cancel.cancel();
}

#[tokio::test]
async fn failed_block_assembler_update_retains_dirty_generation_for_retry() {
    use super::harness::{WorkerSet, harness};
    use crate::block_assembler::BlockAssembler;
    use crate::service::{BlockAssemblerMessage, TxPoolServiceBuilder};
    use ckb_app_config::BlockAssemblerConfig;
    use ckb_jsonrpc_types::ScriptHashType;
    use ckb_util::LinkedHashSet;

    let mut h = harness(1).workers(WorkerSet::None).build();
    let older = h.service.pool.tx_pool.read().await.cloned_snapshot();
    let assembler = BlockAssembler::new(
        BlockAssemblerConfig {
            code_hash: h256!("0x0"),
            args: Default::default(),
            hash_type: ScriptHashType::Data,
            message: Default::default(),
            use_binary_version_as_message_prefix: true,
            binary_version: "TEST".to_string(),
            update_interval_millis: 60_000,
            notify: vec![],
            notify_scripts: vec![],
            notify_timeout_millis: 800,
            notify_auth_token: None,
        },
        Arc::clone(&older),
    )
    .unwrap();
    h.service.block_assembler = Some(assembler);

    let next_block = BlockBuilder::default()
        .parent_hash(older.tip_hash())
        .number(older.tip_number() + 1)
        .epoch(EpochNumberWithFraction::new(0, 0, 1))
        .build();
    let newer = Arc::new(Snapshot::new(
        next_block.header(),
        older.total_difficulty().clone(),
        older.epoch_ext().clone(),
        h.store.store().get_snapshot(),
        Default::default(),
        older.cloned_consensus(),
    ));
    h.service.pool.tx_pool.write().await.snapshot = Arc::clone(&newer);
    h.service
        .journal_block_assembler_message(BlockAssemblerMessage::Pending);

    let mut queue = LinkedHashSet::new();
    assert!(
        !TxPoolServiceBuilder::apply_block_assembler_updates(&h.service, &mut queue).await,
        "tip mismatch must defer rather than acknowledge the proposal update"
    );
    assert_eq!(
        h.service.relay.load_block_assembler_dirty().len(),
        1,
        "failed application retains authoritative dirty work"
    );

    // Restore the matching authoritative snapshot. The next drain must use
    // the retained generation rather than requiring another producer edge.
    h.service.pool.tx_pool.write().await.snapshot = older;
    assert!(TxPoolServiceBuilder::apply_block_assembler_updates(&h.service, &mut queue).await);
    assert!(h.service.relay.load_block_assembler_dirty().is_empty());
}

/// Pool membership and its template delta share one synchronous mutation
/// boundary. In particular, administrative removal must refresh proposals;
/// otherwise an interval-zero assembler can retain a transaction that no
/// longer exists in the pool indefinitely.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commit_and_removal_journal_block_assembler_delta() {
    use super::harness::{WorkerSet, harness};
    use crate::block_assembler::BlockAssembler;
    use crate::service::{BlockAssemblerMessage, RemoveTxOutcome};
    use ckb_app_config::BlockAssemblerConfig;
    use ckb_jsonrpc_types::ScriptHashType;

    let mut h = harness(1).workers(WorkerSet::None).build();
    let snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();
    h.service.block_assembler = Some(
        BlockAssembler::new(
            BlockAssemblerConfig {
                code_hash: h256!("0x0"),
                args: Default::default(),
                hash_type: ScriptHashType::Data,
                message: Default::default(),
                use_binary_version_as_message_prefix: true,
                binary_version: "TEST".to_string(),
                update_interval_millis: 0,
                notify: vec![],
                notify_scripts: vec![],
                notify_timeout_millis: 800,
                notify_auth_token: None,
            },
            snapshot,
        )
        .unwrap(),
    );

    let tx = build_tx(&h.out_points[0], ISSUE_OUTPUT_CAPACITY as usize - 1);
    let tx_hash = tx.hash();
    h.service
        .process_tx(tx, TxSource::Local)
        .await
        .expect("transaction commits");
    let committed = tokio::time::timeout(Duration::from_secs(1), h.block_assembler_rx.recv())
        .await
        .expect("commit journals a template wake")
        .expect("assembler channel remains open");
    assert_eq!(committed, BlockAssemblerMessage::Pending);
    crate::block_assembler::process(h.service.clone(), &committed).await;
    assert_eq!(
        h.service
            .block_assembler
            .as_ref()
            .unwrap()
            .get_current()
            .await
            .proposals
            .len(),
        1
    );

    assert_eq!(h.service.remove_tx(tx_hash).await, RemoveTxOutcome::Removed);
    let removed = tokio::time::timeout(Duration::from_secs(1), h.block_assembler_rx.recv())
        .await
        .expect("removal journals a template wake")
        .expect("assembler channel remains open");
    assert_eq!(removed, BlockAssemblerMessage::Pending);
    crate::block_assembler::process(h.service.clone(), &removed).await;
    assert!(
        h.service
            .block_assembler
            .as_ref()
            .unwrap()
            .get_current()
            .await
            .proposals
            .is_empty()
    );

    h.cancel.cancel();
}

/// Topologically sort dependent transactions so parents come before children.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sort_txs_by_dependencies_orders_parents_before_children() {
    let (_service, _relay, signal, _store, issue_out_points) = service_with_pipeline(1);
    let issue_out_point = &issue_out_points[0];

    let tx_a = build_tx(issue_out_point, 4_000);
    let tx_b = build_tx(&OutPoint::new(tx_a.hash(), 0), 3_000);
    let tx_c = build_tx(&OutPoint::new(tx_b.hash(), 0), 2_000);

    // Shuffle: child first, then grandchild, then parent.
    let mut txs = vec![tx_c.clone(), tx_b.clone(), tx_a.clone()];
    TxPoolService::sort_txs_by_dependencies(&mut txs);

    assert_eq!(txs[0], tx_a);
    assert_eq!(txs[1], tx_b);
    assert_eq!(txs[2], tx_c);

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// A cycle in the dependency graph should keep the original order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sort_txs_by_dependencies_keeps_original_order_on_cycle() {
    let (_service, _relay, signal, _store, issue_out_points) = service_with_pipeline(2);
    let input_a = &issue_out_points[0];
    let input_b = &issue_out_points[1];

    let mut txs = vec![input_a.clone(), input_b.clone()]
        .into_iter()
        .map(|out_point| build_tx(&out_point, 4_000))
        .collect::<Vec<_>>();
    let original = txs.clone();
    TxPoolService::sort_txs_by_dependencies(&mut txs);
    assert_eq!(txs, original);

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// Concurrent RBF replacements for the same input must be ordered by fee.
/// Only the highest-fee candidate should end up in the pool; lower-fee ones
/// must be rejected rather than temporarily displacing the original tx and
/// blocking the higher-fee candidate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_concurrent_rbf_prefers_highest_fee() {
    let (service, _relay, signal, _store, issue_out_points) = service_with_rbf(1);
    let shared_input = &issue_out_points[0];

    // Original tx in pool: fee = 1000 bytes.
    let original = build_tx(shared_input, 4_000);
    let original_id = original.proposal_short_id();
    let original_cycles = measured_cycles(&service, original.clone()).await;
    service
        .submit_remote_tx(
            original,
            TxSource::Remote {
                cycles: original_cycles,
                peer: 1.into(),
            },
        )
        .await
        .unwrap();

    // Wait until the original tx is actually in the pool before racing
    // replacements against it.
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("original tx should be accepted");

    // Replacement candidates with strictly increasing fees.
    let replacements = vec![
        (3_500, 1), // fee 1500
        (3_000, 2), // fee 2000
        (2_500, 3), // fee 2500 (highest)
    ];

    let always_success_script = always_success_script();
    let mut handles = Vec::new();
    let mut ids = Vec::new();
    for (output_capacity, peer) in replacements {
        let tx = TransactionBuilder::default()
            .cell_dep(always_success_dep())
            .input(CellInput::new(shared_input.clone(), 0))
            .output(
                CellOutput::new_builder()
                    .capacity(Capacity::bytes(output_capacity).unwrap())
                    .lock(always_success_script.clone())
                    .build(),
            )
            .output_data(Bytes::default().pack())
            .witness(always_success_script.clone().into_witness())
            .build();
        ids.push((tx.proposal_short_id(), output_capacity));
        let cycles = measured_cycles(&service, tx.clone()).await;
        let svc = service.clone();
        handles.push(tokio::spawn(async move {
            svc.submit_remote_tx(
                tx,
                TxSource::Remote {
                    cycles,
                    peer: peer.into(),
                },
            )
            .await
        }));
    }

    for handle in handles {
        let _ = handle.await.expect("replacement task should not panic");
    }

    // Wait until the pipeline has fully settled and only the highest-fee
    // replacement remains in the pool.  Because remote submissions only block
    // until the tx is enqueued, a lower-fee candidate may briefly enter the
    // pool before a higher-fee candidate that is still racing through the
    // verify/submit stages replaces it.  Polling on the pool contents (not
    // just queue lengths) is required to avoid observing the transient state.
    let expected_id = ids
        .iter()
        .find(|(_, cap)| *cap == 2_500)
        .map(|(id, _)| id)
        .expect("highest-fee replacement exists")
        .clone();

    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let (pending, pipeline_len, settled) = {
                let pool = service.pool.tx_pool.read().await;
                let settled = pool.get_tx_from_pool(&original_id).is_none()
                    && pool.get_tx_from_pool(&expected_id).is_some()
                    && ids
                        .iter()
                        .all(|(id, _)| *id == expected_id || pool.get_tx_from_pool(id).is_none());
                (
                    pool.pool_map.pending_size(),
                    service
                        .pipeline
                        .runtime
                        .read(|coordinator| coordinator.len()),
                    settled,
                )
            };
            if pending == 1 && pipeline_len == 0 && settled {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("pipeline should settle with exactly one RBF replacement accepted");

    let pool = service.pool.tx_pool.read().await;
    assert!(
        pool.get_tx_from_pool(&original_id).is_none(),
        "original tx should have been replaced"
    );
    assert!(
        pool.get_tx_from_pool(&expected_id).is_some(),
        "highest-fee replacement should be in the pool; ids={:?}",
        ids.iter()
            .map(|(id, cap)| (cap, pool.get_tx_from_pool(id).is_some()))
            .collect::<Vec<_>>()
    );

    // All other replacement ids should not be in the pool.
    for (id, cap) in &ids {
        if *id != expected_id {
            assert!(
                pool.get_tx_from_pool(id).is_none(),
                "lower-fee replacement {} should not be in the pool",
                cap
            );
        }
    }

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// A multi-level RBF replacement must recover *all* removed transactions,
/// including descendants, in dependency order. If tx_a is replaced by a
/// higher-fee tx_r that is then rejected by the pool size limit, both tx_a
/// and its descendants tx_b and tx_c must be re-submitted so that parents
/// precede children in the recovery set.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_rbf_rejected_replacement_recovers_descendants_in_order() {
    // Pool size large enough for a small three-tx chain but not for the
    // oversized replacement.
    let (service, _relay, signal, _store, issue_out_points) =
        service_with_rbf_and_max_size(1, 2_000);

    let shared_input = &issue_out_points[0];

    // tx_a -> tx_b -> tx_c
    let tx_a = build_tx(shared_input, 4_998);
    let id_a = tx_a.proposal_short_id();
    let tx_b = build_tx(&OutPoint::new(tx_a.hash(), 0), 4_998);
    let id_b = tx_b.proposal_short_id();
    let tx_c = build_tx(&OutPoint::new(tx_b.hash(), 0), 4_998);
    let id_c = tx_c.proposal_short_id();

    // tx_r spends the same input as tx_a, pays a high enough fee to pass RBF
    // checks, but carries enough output data that it exceeds the tiny pool
    // limit once tx_a has been removed.
    let tx_r = TransactionBuilder::default()
        .cell_dep(always_success_dep())
        .input(CellInput::new(shared_input.clone(), 0))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(2_400).unwrap())
                .lock(always_success_script())
                .build(),
        )
        .output_data(Bytes::from(vec![0u8; 2_000]).pack())
        .build();
    let id_r = tx_r.proposal_short_id();

    let cycles_a = measured_cycles(&service, tx_a.clone()).await;

    // Submit tx_a and wait for it to reach pending so that tx_b can be
    // resolved against the pool.
    service
        .submit_remote_tx(
            tx_a.clone(),
            TxSource::Remote {
                cycles: cycles_a,
                peer: 1.into(),
            },
        )
        .await
        .expect("tx_a should be accepted");

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("tx_a should reach pending");

    let cycles_b = measured_cycles(&service, tx_b.clone()).await;
    service
        .submit_remote_tx(
            tx_b.clone(),
            TxSource::Remote {
                cycles: cycles_b,
                peer: 1.into(),
            },
        )
        .await
        .expect("tx_b should be accepted");

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("tx_b should reach pending");

    let cycles_c = measured_cycles(&service, tx_c.clone()).await;
    service
        .submit_remote_tx(
            tx_c.clone(),
            TxSource::Remote {
                cycles: cycles_c,
                peer: 1.into(),
            },
        )
        .await
        .expect("tx_c should be accepted");

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("tx_c should reach pending");

    let cycles_r = measured_cycles(&service, tx_r.clone()).await;

    // Submit the oversized replacement.
    let _ = service
        .submit_remote_tx(
            tx_r.clone(),
            TxSource::Remote {
                cycles: cycles_r,
                peer: 1.into(),
            },
        )
        .await;

    // Wait for the pipeline to drain and the original chain to be recovered.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (a_in_pool, b_in_pool, c_in_pool, r_in_pool, pipeline_len) = {
                let pool = service.pool.tx_pool.read().await;
                (
                    pool.get_tx_from_pool(&id_a).is_some(),
                    pool.get_tx_from_pool(&id_b).is_some(),
                    pool.get_tx_from_pool(&id_c).is_some(),
                    pool.get_tx_from_pool(&id_r).is_some(),
                    service
                        .pipeline
                        .runtime
                        .read(|coordinator| coordinator.len()),
                )
            };
            if a_in_pool && b_in_pool && c_in_pool && !r_in_pool && pipeline_len == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("original chain should be recovered after rejected RBF replacement");

    let pool = service.pool.tx_pool.read().await;
    assert!(
        pool.get_tx_from_pool(&id_r).is_none(),
        "oversized replacement should be rejected"
    );
    assert!(
        pool.get_tx_from_pool(&id_a).is_some(),
        "tx_a should be recovered"
    );
    assert!(
        pool.get_tx_from_pool(&id_b).is_some(),
        "tx_b (descendant) should be recovered"
    );
    assert!(
        pool.get_tx_from_pool(&id_c).is_some(),
        "tx_c (grand-descendant) should be recovered"
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// A successful RBF keeps the removed dependency tree in the historical
/// conflict cache. Removing the replacement first frees only the original
/// parent's confirmed input; each recovered parent acceptance must then make
/// its newly available outputs drive the next cached descendant. Without that
/// accepted-output event, the parent returns while child and grandchild remain
/// cache-owned forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn successful_rbf_recovery_cascades_from_accepted_parent_outputs() {
    let (service, _relay, signal, _store, issue_out_points) = service_with_rbf(1);
    let shared_input = &issue_out_points[0];

    let parent = build_tx(shared_input, 4_998);
    let child = build_tx(&OutPoint::new(parent.hash(), 0), 4_996);
    let grandchild = build_tx(&OutPoint::new(child.hash(), 0), 4_994);
    let original = [parent.clone(), child.clone(), grandchild.clone()];

    for (index, tx) in original.iter().enumerate() {
        let cycles = measured_cycles(&service, tx.clone()).await;
        service
            .submit_remote_tx(
                tx.clone(),
                TxSource::Remote {
                    cycles,
                    peer: 1.into(),
                },
            )
            .await
            .expect("original dependency entry should enqueue");
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if service
                    .pool
                    .tx_pool
                    .read()
                    .await
                    .get_tx_from_pool(&tx.proposal_short_id())
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("original dependency entry {index} should reach the pool"));
    }

    let replacement = build_tx(shared_input, 4_900);
    let replacement_id = replacement.proposal_short_id();
    let replacement_cycles = measured_cycles(&service, replacement.clone()).await;
    service
        .submit_remote_tx(
            replacement.clone(),
            TxSource::Remote {
                cycles: replacement_cycles,
                peer: 2.into(),
            },
        )
        .await
        .expect("higher-fee replacement should enqueue");

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let settled = {
                let pool = service.pool.tx_pool.read().await;
                pool.get_tx_from_pool(&replacement_id).is_some()
                    && original.iter().all(|tx| {
                        pool.get_tx_from_pool(&tx.proposal_short_id()).is_none()
                            && pool.conflict_cache.contains_hash(&tx.hash())
                    })
            };
            if settled {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("successful RBF should move the complete original tree to history");

    assert_eq!(
        service.remove_tx(replacement.hash()).await,
        crate::service::RemoveTxOutcome::Removed
    );

    let recovered = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let recovered = {
                let pool = service.pool.tx_pool.read().await;
                original.iter().all(|tx| {
                    pool.get_tx_from_pool(&tx.proposal_short_id()).is_some()
                        && !pool.conflict_cache.contains_hash(&tx.hash())
                }) && pool.get_tx_from_pool(&replacement_id).is_none()
            };
            if recovered
                && service
                    .pipeline
                    .runtime
                    .read(|coordinator| coordinator.len() == 0)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    if recovered.is_err() {
        let pool = service.pool.tx_pool.read().await;
        let locations = original
            .iter()
            .map(|tx| {
                let id = tx.proposal_short_id();
                (
                    tx.hash(),
                    pool.get_tx_from_pool(&id).is_some(),
                    pool.conflict_cache.contains_hash(&tx.hash()),
                    service
                        .pipeline
                        .runtime
                        .read(|coordinator| coordinator.view(&tx.hash()).map(|view| view.location)),
                )
            })
            .collect::<Vec<_>>();
        panic!(
            "accepted parent outputs should recover the complete cached tree: locations={locations:?}, recovery={}, discovery={}",
            pool.conflict_recovery_len(),
            pool.conflict_discovery_len(),
        );
    }

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// A retained (detached) tx that is *already back in the pool* must be
/// treated as recovered, not as a failure: cascading on `Duplicated` would
/// evict its healthy dependents and emit spurious Dead rejections (this is
/// also what a retried reorg sees on its second pass).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reorg_retain_duplicate_does_not_cascade_dependents() {
    use std::collections::{HashSet, VecDeque};

    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline(1);
    let issue_out_point = &issue_out_points[0];

    // Parent and its child, both pending in the pool.
    let parent = build_tx(issue_out_point, 4_000);
    let parent_output = OutPoint::new(parent.hash(), 0);
    let child = build_tx(&parent_output, 3_000);

    // Child first (it parks in the ordered queue), then the parent. The
    // child cannot be cycle-measured until the parent is in the pool, so
    // reuse the parent's (identical always-success script).
    let cycles = measured_cycles(&service, parent.clone()).await;
    for tx in [&child, &parent] {
        service
            .submit_remote_tx(
                tx.clone(),
                TxSource::Remote {
                    cycles,
                    peer: 1.into(),
                },
            )
            .await
            .expect("enqueue remote tx should succeed");
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("parent and child should be pending before reorg");

    // A detached block containing the parent: the retain loop re-adds it and
    // hits `Duplicated` (it never left the pool). Pre-fix this cascaded and
    // evicted the child with a spurious Dead rejection.
    let detached_block = BlockBuilder::default()
        .number(1)
        .parent_hash(service.pool.tx_pool.read().await.snapshot.tip_hash())
        .epoch(EpochNumberWithFraction::new(0, 0, 1).full_value())
        .transaction(
            TransactionBuilder::default()
                .input(CellInput::new(OutPoint::null(), 0))
                .output(
                    CellOutput::new_builder()
                        .capacity(Capacity::bytes(1_000).unwrap())
                        .build(),
                )
                .output_data(Bytes::default().pack())
                .build(),
        )
        .transaction(parent.clone())
        .build();

    let snapshot = service.pool.tx_pool.read().await.cloned_snapshot();
    service
        .update_tx_pool_for_reorg(
            [detached_block].into(),
            VecDeque::new(),
            HashSet::new(),
            snapshot,
        )
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;
    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(
        pending, 2,
        "Duplicated retain must not cascade-remove the child"
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}
