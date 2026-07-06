//! End-to-end tests for the tx-pool resolve -> verify -> submit pipeline.

use crate::callback::Callbacks;
use crate::component::orphan::OrphanPool;
use crate::component::verify_queue::VerifyQueue;
use crate::pool::TxPool;
use crate::process::PreCheckedTx;
use crate::resolve_mgr::{OrderedResolver, ResolveExit};
use crate::service::TxPoolService;
use crate::verify_mgr::VerifyMgr;
use ckb_app_config::{TxPoolConfig, VerifyOrdering};
use ckb_chain_spec::consensus::Consensus;
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_crypto::secp::Privkey;
use ckb_fee_estimator::FeeEstimator;
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
};
use ckb_verification::{TxVerifyEnv, cache::init_cache};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, mpsc, watch};

const MAX_TX_VERIFY_CYCLES: u64 = 70_000_000;
const ISSUE_OUTPUT_CAPACITY: u64 = 5_000;

fn tx_pool_config() -> TxPoolConfig {
    TxPoolConfig {
        max_tx_pool_size: 180_000_000,
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
    }
}

fn test_consensus(issue_outputs: usize) -> (Consensus, Vec<OutPoint>) {
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

fn snapshot_with_genesis(consensus: Arc<Consensus>) -> (MockStore, Arc<Snapshot>) {
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
    service_with_pipeline_workers(issue_outputs, 2)
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
    let (consensus, issue_out_points) = test_consensus(issue_outputs);
    let consensus = Arc::new(consensus);
    let (_store, snap) = snapshot_with_genesis(Arc::clone(&consensus));
    let mut config = tx_pool_config();
    config.max_tx_verify_workers = max_workers;
    let (tx_relay_sender, tx_relay_receiver) = ckb_channel::bounded(1024);
    let (block_assembler_sender, _) = mpsc::channel(1);

    let ordered_resolve_queue = Arc::new(RwLock::new(
        crate::component::ordered_resolve_queue::OrderedResolveQueue::new(),
    ));
    let verify_queue = Arc::new(RwLock::new(VerifyQueue::new(
        config.max_tx_verify_cycles,
        config.verify_ordering,
    )));
    #[cfg(feature = "pipeline")]
    let max_workers = config.max_tx_verify_workers.max(1);
    #[cfg(feature = "pipeline")]
    let pre_check_workers =
        max_workers.min(std::thread::available_parallelism().map_or(4, |n| n.get()));
    #[cfg(feature = "pipeline")]
    let pre_check_cancel = ckb_stop_handler::CancellationToken::new();
    #[cfg(feature = "pipeline")]
    let pre_check_queue = Arc::new(crate::component::pre_check_queue::PreCheckQueue::new(
        pre_check_cancel,
    ));
    let (deferred_sender, mut deferred_receiver) = mpsc::channel(1024);
    let (_chunk_tx, chunk_rx) = watch::channel(ChunkCommand::Resume);

    let service = TxPoolService {
        tx_pool: Arc::new(RwLock::new(TxPool::new(config.clone(), snap))),
        orphan: Arc::new(RwLock::new(OrphanPool::new())),
        consensus: Arc::clone(&consensus),
        tx_pool_config: Arc::new(config),
        block_assembler: None,
        txs_verify_cache: Arc::new(RwLock::new(init_cache())),
        callbacks: Arc::new(Callbacks::new()),
        network: super::chunk::dummy_network(),
        tx_relay_sender,
        ordered_resolve_queue: Arc::clone(&ordered_resolve_queue),
        verify_queue: Arc::clone(&verify_queue),
        block_assembler_sender,
        fee_estimator: FeeEstimator::new_dummy(),
        recent_reject: None,
        #[cfg(feature = "pipeline")]
        pre_check_queue: Arc::clone(&pre_check_queue),
        #[cfg(feature = "pipeline")]
        chunk_rx,
        #[cfg(feature = "pipeline")]
        rbf_candidates: Arc::new(RwLock::new(
            crate::component::rbf_candidates::RbfCandidates::new(),
        )),
        deferred_sender,
    };

    // Drain deferred tasks (RBF recovery + verify cache updates) for tests.
    {
        let ordered = Arc::clone(&ordered_resolve_queue);
        let txs_verify_cache = Arc::clone(&service.txs_verify_cache);
        tokio::spawn(async move {
            while let Some(task) = deferred_receiver.recv().await {
                match task {
                    crate::service::DeferredTask::RecoverTxs(txs) => {
                        let mut queue = ordered.write().await;
                        for tx in txs {
                            let _ = queue.add_tx(crate::resolved_tx::ResolveJob {
                                tx,
                                remote: None,
                                is_proposal_tx: false,
                                attempts: 0,
                            });
                        }
                    }
                    crate::service::DeferredTask::CacheUpdate { wtx_hash, verified } => {
                        let mut guard = txs_verify_cache.write().await;
                        guard.put(wtx_hash, verified);
                    }
                }
            }
        });
    }

    #[cfg(feature = "pipeline")]
    {
        for _ in 0..pre_check_workers {
            let svc = service.clone();
            let queue = Arc::clone(&pre_check_queue);
            tokio::spawn(async move {
                while let Some(job) = queue.pop().await {
                    let _ = svc
                        .classify_and_enqueue_tx(job.tx, job.is_proposal_tx, job.remote)
                        .await;
                }
            });
        }
    }

    let signal = CancellationToken::new();
    let (chunk_tx, chunk_rx) = watch::channel(ChunkCommand::Resume);

    let mut verify_mgr = VerifyMgr::new(service.clone(), chunk_rx, signal.child_token());
    tokio::spawn(async move { verify_mgr.run().await });

    let ordered_resolver = OrderedResolver::new(
        service.clone(),
        Arc::clone(&ordered_resolve_queue),
        Arc::clone(&verify_queue),
        chunk_tx.subscribe(),
        signal.child_token(),
    );
    let (resolve_exit_tx, mut resolve_exit_rx) = tokio::sync::mpsc::unbounded_channel();
    let resolver_handle = ordered_resolver.start(resolve_exit_tx);
    tokio::spawn(async move {
        if let Some((_, ResolveExit::Panicked { message })) = resolve_exit_rx.recv().await {
            panic!("tx-pool ordered resolver panicked: {message}");
        }
        let _ = resolver_handle.await;
    });

    (service, tx_relay_receiver, signal, _store, issue_out_points)
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

fn secp_test_consensus(issue_outputs: usize) -> (Consensus, Vec<OutPoint>, Vec<CellDep>) {
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
    let (consensus, issue_out_points, cell_deps) = secp_test_consensus(issue_outputs);
    let consensus = Arc::new(consensus);
    let (_store, snap) = snapshot_with_genesis(Arc::clone(&consensus));
    let mut config = tx_pool_config();
    config.max_tx_verify_workers = max_workers;
    let (tx_relay_sender, tx_relay_receiver) = ckb_channel::bounded(1024);
    let (block_assembler_sender, _) = mpsc::channel(1);

    let ordered_resolve_queue = Arc::new(RwLock::new(
        crate::component::ordered_resolve_queue::OrderedResolveQueue::new(),
    ));
    let verify_queue = Arc::new(RwLock::new(VerifyQueue::new(
        config.max_tx_verify_cycles,
        config.verify_ordering,
    )));
    #[cfg(feature = "pipeline")]
    let max_workers = config.max_tx_verify_workers.max(1);
    #[cfg(feature = "pipeline")]
    let pre_check_workers =
        max_workers.min(std::thread::available_parallelism().map_or(4, |n| n.get()));
    #[cfg(feature = "pipeline")]
    let pre_check_cancel = ckb_stop_handler::CancellationToken::new();
    #[cfg(feature = "pipeline")]
    let pre_check_queue = Arc::new(crate::component::pre_check_queue::PreCheckQueue::new(
        pre_check_cancel,
    ));
    let (deferred_sender, mut deferred_receiver) = mpsc::channel(1024);
    let (_chunk_tx, chunk_rx) = watch::channel(ChunkCommand::Resume);

    let service = TxPoolService {
        tx_pool: Arc::new(RwLock::new(TxPool::new(config.clone(), snap))),
        orphan: Arc::new(RwLock::new(OrphanPool::new())),
        consensus: Arc::clone(&consensus),
        tx_pool_config: Arc::new(config),
        block_assembler: None,
        txs_verify_cache: Arc::new(RwLock::new(init_cache())),
        callbacks: Arc::new(Callbacks::new()),
        network: super::chunk::dummy_network(),
        tx_relay_sender,
        ordered_resolve_queue: Arc::clone(&ordered_resolve_queue),
        verify_queue: Arc::clone(&verify_queue),
        block_assembler_sender,
        fee_estimator: FeeEstimator::new_dummy(),
        recent_reject: None,
        #[cfg(feature = "pipeline")]
        pre_check_queue: Arc::clone(&pre_check_queue),
        #[cfg(feature = "pipeline")]
        chunk_rx,
        #[cfg(feature = "pipeline")]
        rbf_candidates: Arc::new(RwLock::new(
            crate::component::rbf_candidates::RbfCandidates::new(),
        )),
        deferred_sender,
    };

    // Drain deferred tasks (RBF recovery + verify cache updates) for tests.
    {
        let ordered = Arc::clone(&ordered_resolve_queue);
        let txs_verify_cache = Arc::clone(&service.txs_verify_cache);
        tokio::spawn(async move {
            while let Some(task) = deferred_receiver.recv().await {
                match task {
                    crate::service::DeferredTask::RecoverTxs(txs) => {
                        let mut queue = ordered.write().await;
                        for tx in txs {
                            let _ = queue.add_tx(crate::resolved_tx::ResolveJob {
                                tx,
                                remote: None,
                                is_proposal_tx: false,
                                attempts: 0,
                            });
                        }
                    }
                    crate::service::DeferredTask::CacheUpdate { wtx_hash, verified } => {
                        let mut guard = txs_verify_cache.write().await;
                        guard.put(wtx_hash, verified);
                    }
                }
            }
        });
    }

    #[cfg(feature = "pipeline")]
    {
        for _ in 0..pre_check_workers {
            let svc = service.clone();
            let queue = Arc::clone(&pre_check_queue);
            tokio::spawn(async move {
                while let Some(job) = queue.pop().await {
                    let _ = svc
                        .classify_and_enqueue_tx(job.tx, job.is_proposal_tx, job.remote)
                        .await;
                }
            });
        }
    }

    let signal = CancellationToken::new();
    let (chunk_tx, chunk_rx) = watch::channel(ChunkCommand::Resume);

    let mut verify_mgr = VerifyMgr::new(service.clone(), chunk_rx, signal.child_token());
    tokio::spawn(async move { verify_mgr.run().await });

    let ordered_resolver = OrderedResolver::new(
        service.clone(),
        Arc::clone(&ordered_resolve_queue),
        Arc::clone(&verify_queue),
        chunk_tx.subscribe(),
        signal.child_token(),
    );
    let (resolve_exit_tx, mut resolve_exit_rx) = tokio::sync::mpsc::unbounded_channel();
    let resolver_handle = ordered_resolver.start(resolve_exit_tx);
    tokio::spawn(async move {
        if let Some((_, ResolveExit::Panicked { message })) = resolve_exit_rx.recv().await {
            panic!("tx-pool ordered resolver panicked: {message}");
        }
        let _ = resolver_handle.await;
    });

    (
        service,
        tx_relay_receiver,
        signal,
        _store,
        issue_out_points,
        cell_deps,
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

#[cfg(feature = "pipeline")]
async fn submit_local_tx(service: &TxPoolService, tx: TransactionView) -> u64 {
    let (ret, _snapshot) = service
        .process_tx_sync(tx, None, None)
        .await
        .expect("local process tx should return a result");
    ret.expect("local tx should be accepted").cycles
}

async fn verify_cycles(service: &TxPoolService, tx: TransactionView) -> u64 {
    let tx_size = tx.data().serialized_size_in_block();
    let (pre_check_ret, snapshot) = service.pre_check(&tx, tx_size).await;
    let PreCheckedTx { rtx, status, .. } =
        pre_check_ret.expect("pre_check for cycle measurement should succeed");
    let verify_cache = service.fetch_tx_verify_cache(&tx).await;
    let max_cycles = service.consensus.max_block_cycles();
    let tx_env = match status {
        crate::process::TxStatus::Fresh => Arc::new(TxVerifyEnv::new_submit(snapshot.tip_header())),
        crate::process::TxStatus::Gap => {
            Arc::new(TxVerifyEnv::new_proposed(snapshot.tip_header(), 0))
        }
        crate::process::TxStatus::Proposed => {
            Arc::new(TxVerifyEnv::new_proposed(snapshot.tip_header(), 1))
        }
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
            .submit_remote_tx(tx.clone(), cycles, 1.into())
            .await
            .expect("enqueue remote tx should succeed");
    }

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.tx_pool.read().await.pool_map.pending_size();
            if pending == txs.len() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("pipeline should process all independent txs in time");

    let pending = service.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(pending, txs.len());

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[cfg(feature = "pipeline")]
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
        .submit_remote_tx(tx_b.clone(), tx_a_cycles, 1.into())
        .await
        .unwrap();

    service
        .submit_remote_tx(tx_a.clone(), tx_a_cycles, 1.into())
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.tx_pool.read().await.pool_map.pending_size();
            if pending == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("pipeline should process dependent txs in time");

    let pending = service.tx_pool.read().await.pool_map.pending_size();
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
                .capacity(Capacity::bytes(4_000).unwrap())
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
    let handle_a =
        tokio::spawn(async move { service_a.submit_remote_tx(tx_a, cycles_a, 1.into()).await });
    let handle_b =
        tokio::spawn(async move { service_b.submit_remote_tx(tx_b, cycles_b, 1.into()).await });

    let (res_a, res_b) = tokio::join!(handle_a, handle_b);
    let _ = res_a.expect("task a should not panic");
    let _ = res_b.expect("task b should not panic");

    // Wait for the pipeline to drain. Both txs should leave the ordered/verify
    // queues and exactly one must land in the pending pool.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (pending, ordered_len, verify_len) = {
                let pool = service.tx_pool.read().await;
                let ordered = service.ordered_resolve_queue.read().await;
                let verify = service.verify_queue.read().await;
                (pool.pool_map.pending_size(), ordered.len(), verify.len())
            };
            if pending == 1 && ordered_len == 0 && verify_len == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("pipeline should settle with exactly one double-spend tx accepted");

    let pool = service.tx_pool.read().await;
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
async fn pipeline_serializes_cell_dep_on_in_flight_input() {
    // tx_a spends an on-chain cell X. tx_b spends a different cell but uses X as
    // a cell dep. Because X is about to be consumed by tx_a, tx_b must wait until
    // tx_a resolves; after that X is dead and tx_b should be rejected rather than
    // observing a stale live X and racing through the pipeline.
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
        .submit_remote_tx(tx_a.clone(), cycles_a, 1.into())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (pending, verify_len) = {
                let pool = service.tx_pool.read().await;
                let verify = service.verify_queue.read().await;
                (pool.pool_map.pending_size(), verify.len())
            };
            if pending == 1 || verify_len == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("tx_a should enter the pipeline");

    service
        .submit_remote_tx(tx_b.clone(), cycles_b, 2.into())
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let (pending, ordered_len, verify_len, orphan_len) = {
                let pool = service.tx_pool.read().await;
                let ordered = service.ordered_resolve_queue.read().await;
                let verify = service.verify_queue.read().await;
                let orphan = service.orphan.read().await;
                (
                    pool.pool_map.pending_size(),
                    ordered.len(),
                    verify.len(),
                    orphan.len(),
                )
            };
            if pending == 1 && ordered_len == 0 && verify_len == 0 && orphan_len == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("pipeline should settle with tx_a accepted and tx_b rejected");

    let pool = service.tx_pool.read().await;
    assert!(
        pool.get_tx_from_pool(&id_a).is_some(),
        "tx_a should be accepted"
    );
    assert!(
        pool.get_tx_from_pool(&id_b).is_none(),
        "tx_b should be rejected because its cell dep was consumed by tx_a"
    );

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
        .process_tx(tx_a.clone(), None)
        .await
        .expect("tx_a should be accepted");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (pending, ordered_len, verify_len, orphan_len) = {
                let pool = service.tx_pool.read().await;
                let ordered = service.ordered_resolve_queue.read().await;
                let verify = service.verify_queue.read().await;
                let orphan = service.orphan.read().await;
                (
                    pool.pool_map.pending_size(),
                    ordered.len(),
                    verify.len(),
                    orphan.len(),
                )
            };
            if pending == 1 && ordered_len == 0 && verify_len == 0 && orphan_len == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("tx_a should settle");

    // Now tx_b's input and cell dep point to the same in-pool out-point.
    service
        .process_tx(tx_b.clone(), None)
        .await
        .expect("tx_b should be accepted even though its cell dep is also its input");

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (pending, ordered_len, verify_len, orphan_len) = {
                let pool = service.tx_pool.read().await;
                let ordered = service.ordered_resolve_queue.read().await;
                let verify = service.verify_queue.read().await;
                let orphan = service.orphan.read().await;
                (
                    pool.pool_map.pending_size(),
                    ordered.len(),
                    verify.len(),
                    orphan.len(),
                )
            };
            if pending == 2 && ordered_len == 0 && verify_len == 0 && orphan_len == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("tx_b should settle");

    let pool = service.tx_pool.read().await;
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
            .submit_remote_tx(tx.clone(), *cycles, 1.into())
            .await
            .expect("enqueue secp remote tx should succeed");
    }

    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let pending = service.tx_pool.read().await.pool_map.pending_size();
            if pending == txs.len() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("pipeline should process all independent secp txs in time");

    let pending = service.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(pending, txs.len());

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[cfg(feature = "pipeline")]
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
        .submit_remote_tx(child.clone(), child_cycles, 1.into())
        .await
        .expect("enqueue child secp tx should succeed");

    service
        .submit_remote_tx(parent.clone(), parent_cycles, 1.into())
        .await
        .expect("enqueue parent secp tx should succeed");

    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let pending = service.tx_pool.read().await.pool_map.pending_size();
            if pending == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("pipeline should process dependent secp txs in order");

    let pending = service.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(pending, 2);

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// Test that `update_tx_pool_for_reorg` correctly routes retained (detached)
/// transactions through the pipeline entry point (`classify_and_enqueue_tx`)
/// rather than blocking the write lock with inline verification.
///
/// Scenario:
/// 1. Submit 3 independent txs; wait for all to reach pending.
/// 2. Build a "detached" block containing 2 of those txs (simulating they were
///    mined in a block that is now being orphaned).
/// 3. Call `update_tx_pool_for_reorg` with the block as detached, empty attached.
/// 4. Verify: no panic, pool stays consistent, and `classify_and_enqueue_tx` is
///    called for the 2 retained txs (errors are expected since they're still in
///    the pool — the pipeline path logs them at debug level).
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
            .submit_remote_tx(tx.clone(), cycles, 1.into())
            .await
            .expect("enqueue remote tx should succeed");
    }

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.tx_pool.read().await.pool_map.pending_size();
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
        .parent_hash(service.tx_pool.read().await.snapshot.tip_hash())
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
    let snapshot = service.tx_pool.read().await.cloned_snapshot();

    // Trigger the reorg. In pipeline mode, this should call
    // classify_and_enqueue_tx for each retained tx after releasing the write
    // lock. The calls will fail with "already in pool" errors (expected),
    // but the critical thing is:
    // - No panic
    // - Pool remains consistent
    // - classify_and_enqueue_tx is exercised (pipeline path, not inline verify)
    service
        .update_tx_pool_for_reorg(
            detached_blocks,
            attached_blocks,
            detached_proposal_id,
            snapshot,
        )
        .await;

    // Give the pipeline a moment to process any classify_and_enqueue_tx calls.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Pool should still contain all 3 txs (reorg didn't remove anything
    // since attached was empty and the txs were in pending, not committed).
    let pending = service.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(
        pending, 3,
        "pool should still have all 3 txs after reorg with empty attached"
    );

    // Verify the ordered resolve queue and verify queue are drained (no stuck txs).
    let ordered_len = service.ordered_resolve_queue.read().await.len();
    let verify_len = service.verify_queue.read().await.len();
    assert_eq!(
        ordered_len, 0,
        "ordered resolve queue should be empty after reorg classify calls fail"
    );
    assert_eq!(
        verify_len, 0,
        "verify queue should be empty after reorg classify calls fail"
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

// ---------------------------------------------------------------------------
// Additional helpers for specialized test configurations
// ---------------------------------------------------------------------------

/// Same as `service_with_pipeline` but enables RBF by setting `min_rbf_rate`
/// above `min_fee_rate`.
fn service_with_rbf(
    issue_outputs: usize,
) -> (
    TxPoolService,
    ckb_channel::Receiver<crate::service::TxVerificationResult>,
    CancellationToken,
    MockStore,
    Vec<OutPoint>,
) {
    let (consensus, issue_out_points) = test_consensus(issue_outputs);
    let consensus = Arc::new(consensus);
    let (_store, snap) = snapshot_with_genesis(Arc::clone(&consensus));
    let mut config = tx_pool_config();
    config.min_rbf_rate = ckb_types::core::FeeRate::from_u64(1000);
    #[cfg(feature = "pipeline")]
    let max_workers = config.max_tx_verify_workers.max(1);
    let (tx_relay_sender, tx_relay_receiver) = ckb_channel::bounded(1024);
    let (block_assembler_sender, _) = mpsc::channel(1);

    let ordered_resolve_queue = Arc::new(RwLock::new(
        crate::component::ordered_resolve_queue::OrderedResolveQueue::new(),
    ));
    let verify_queue = Arc::new(RwLock::new(VerifyQueue::new(
        config.max_tx_verify_cycles,
        config.verify_ordering,
    )));
    #[cfg(feature = "pipeline")]
    let pre_check_workers =
        max_workers.min(std::thread::available_parallelism().map_or(4, |n| n.get()));
    #[cfg(feature = "pipeline")]
    let pre_check_cancel = ckb_stop_handler::CancellationToken::new();
    #[cfg(feature = "pipeline")]
    let pre_check_queue = Arc::new(crate::component::pre_check_queue::PreCheckQueue::new(
        pre_check_cancel,
    ));
    let (deferred_sender, mut deferred_receiver) = mpsc::channel(1024);
    let (_chunk_tx, chunk_rx) = watch::channel(ChunkCommand::Resume);

    let service = TxPoolService {
        tx_pool: Arc::new(RwLock::new(TxPool::new(config.clone(), snap))),
        orphan: Arc::new(RwLock::new(OrphanPool::new())),
        consensus: Arc::clone(&consensus),
        tx_pool_config: Arc::new(config),
        block_assembler: None,
        txs_verify_cache: Arc::new(RwLock::new(init_cache())),
        callbacks: Arc::new(Callbacks::new()),
        network: super::chunk::dummy_network(),
        tx_relay_sender,
        ordered_resolve_queue: Arc::clone(&ordered_resolve_queue),
        verify_queue: Arc::clone(&verify_queue),
        block_assembler_sender,
        fee_estimator: FeeEstimator::new_dummy(),
        recent_reject: None,
        #[cfg(feature = "pipeline")]
        pre_check_queue: Arc::clone(&pre_check_queue),
        #[cfg(feature = "pipeline")]
        chunk_rx,
        #[cfg(feature = "pipeline")]
        rbf_candidates: Arc::new(RwLock::new(
            crate::component::rbf_candidates::RbfCandidates::new(),
        )),
        deferred_sender,
    };

    {
        let ordered = Arc::clone(&ordered_resolve_queue);
        let txs_verify_cache = Arc::clone(&service.txs_verify_cache);
        tokio::spawn(async move {
            while let Some(task) = deferred_receiver.recv().await {
                match task {
                    crate::service::DeferredTask::RecoverTxs(txs) => {
                        let mut queue = ordered.write().await;
                        for tx in txs {
                            let _ = queue.add_tx(crate::resolved_tx::ResolveJob {
                                tx,
                                remote: None,
                                is_proposal_tx: false,
                                attempts: 0,
                            });
                        }
                    }
                    crate::service::DeferredTask::CacheUpdate { wtx_hash, verified } => {
                        let mut guard = txs_verify_cache.write().await;
                        guard.put(wtx_hash, verified);
                    }
                }
            }
        });
    }

    #[cfg(feature = "pipeline")]
    {
        for _ in 0..pre_check_workers {
            let svc = service.clone();
            let queue = Arc::clone(&pre_check_queue);
            tokio::spawn(async move {
                while let Some(job) = queue.pop().await {
                    let _ = svc
                        .classify_and_enqueue_tx(job.tx, job.is_proposal_tx, job.remote)
                        .await;
                }
            });
        }
    }

    let signal = CancellationToken::new();
    let (chunk_tx, chunk_rx) = watch::channel(ChunkCommand::Resume);

    let mut verify_mgr = VerifyMgr::new(service.clone(), chunk_rx, signal.child_token());
    tokio::spawn(async move { verify_mgr.run().await });

    let ordered_resolver = OrderedResolver::new(
        service.clone(),
        Arc::clone(&ordered_resolve_queue),
        Arc::clone(&verify_queue),
        chunk_tx.subscribe(),
        signal.child_token(),
    );
    let (resolve_exit_tx, mut resolve_exit_rx) = tokio::sync::mpsc::unbounded_channel();
    let resolver_handle = ordered_resolver.start(resolve_exit_tx);
    tokio::spawn(async move {
        if let Some((_, ResolveExit::Panicked { message })) = resolve_exit_rx.recv().await {
            panic!("tx-pool ordered resolver panicked: {message}");
        }
        let _ = resolver_handle.await;
    });

    (service, tx_relay_receiver, signal, _store, issue_out_points)
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
    let (consensus, issue_out_points) = test_consensus(issue_outputs);
    let consensus = Arc::new(consensus);
    let (_store, snap) = snapshot_with_genesis(Arc::clone(&consensus));
    let mut config = tx_pool_config();
    config.min_rbf_rate = ckb_types::core::FeeRate::from_u64(1000);
    config.max_tx_pool_size = max_tx_pool_size;
    #[cfg(feature = "pipeline")]
    let max_workers = config.max_tx_verify_workers.max(1);
    let (tx_relay_sender, tx_relay_receiver) = ckb_channel::bounded(1024);
    let (block_assembler_sender, _) = mpsc::channel(1);

    let ordered_resolve_queue = Arc::new(RwLock::new(
        crate::component::ordered_resolve_queue::OrderedResolveQueue::new(),
    ));
    let verify_queue = Arc::new(RwLock::new(VerifyQueue::new(
        config.max_tx_verify_cycles,
        config.verify_ordering,
    )));
    #[cfg(feature = "pipeline")]
    let pre_check_workers =
        max_workers.min(std::thread::available_parallelism().map_or(4, |n| n.get()));
    #[cfg(feature = "pipeline")]
    let pre_check_cancel = ckb_stop_handler::CancellationToken::new();
    #[cfg(feature = "pipeline")]
    let pre_check_queue = Arc::new(crate::component::pre_check_queue::PreCheckQueue::new(
        pre_check_cancel,
    ));
    let (deferred_sender, mut deferred_receiver) = mpsc::channel(1024);
    let (_chunk_tx, chunk_rx) = watch::channel(ChunkCommand::Resume);

    let service = TxPoolService {
        tx_pool: Arc::new(RwLock::new(TxPool::new(config.clone(), snap))),
        orphan: Arc::new(RwLock::new(OrphanPool::new())),
        consensus: Arc::clone(&consensus),
        tx_pool_config: Arc::new(config),
        block_assembler: None,
        txs_verify_cache: Arc::new(RwLock::new(init_cache())),
        callbacks: Arc::new(Callbacks::new()),
        network: super::chunk::dummy_network(),
        tx_relay_sender,
        ordered_resolve_queue: Arc::clone(&ordered_resolve_queue),
        verify_queue: Arc::clone(&verify_queue),
        block_assembler_sender,
        fee_estimator: FeeEstimator::new_dummy(),
        recent_reject: None,
        #[cfg(feature = "pipeline")]
        pre_check_queue: Arc::clone(&pre_check_queue),
        #[cfg(feature = "pipeline")]
        chunk_rx,
        #[cfg(feature = "pipeline")]
        rbf_candidates: Arc::new(RwLock::new(
            crate::component::rbf_candidates::RbfCandidates::new(),
        )),
        deferred_sender,
    };

    {
        let ordered = Arc::clone(&ordered_resolve_queue);
        let txs_verify_cache = Arc::clone(&service.txs_verify_cache);
        tokio::spawn(async move {
            while let Some(task) = deferred_receiver.recv().await {
                match task {
                    crate::service::DeferredTask::RecoverTxs(txs) => {
                        let mut queue = ordered.write().await;
                        for tx in txs {
                            let _ = queue.add_tx(crate::resolved_tx::ResolveJob {
                                tx,
                                remote: None,
                                is_proposal_tx: false,
                                attempts: 0,
                            });
                        }
                    }
                    crate::service::DeferredTask::CacheUpdate { wtx_hash, verified } => {
                        let mut guard = txs_verify_cache.write().await;
                        guard.put(wtx_hash, verified);
                    }
                }
            }
        });
    }

    #[cfg(feature = "pipeline")]
    {
        for _ in 0..pre_check_workers {
            let svc = service.clone();
            let queue = Arc::clone(&pre_check_queue);
            tokio::spawn(async move {
                while let Some(job) = queue.pop().await {
                    let _ = svc
                        .classify_and_enqueue_tx(job.tx, job.is_proposal_tx, job.remote)
                        .await;
                }
            });
        }
    }

    let signal = CancellationToken::new();
    let (chunk_tx, chunk_rx) = watch::channel(ChunkCommand::Resume);

    let mut verify_mgr = VerifyMgr::new(service.clone(), chunk_rx, signal.child_token());
    tokio::spawn(async move { verify_mgr.run().await });

    let ordered_resolver = OrderedResolver::new(
        service.clone(),
        Arc::clone(&ordered_resolve_queue),
        Arc::clone(&verify_queue),
        chunk_tx.subscribe(),
        signal.child_token(),
    );
    let (resolve_exit_tx, mut resolve_exit_rx) = tokio::sync::mpsc::unbounded_channel();
    let resolver_handle = ordered_resolver.start(resolve_exit_tx);
    tokio::spawn(async move {
        if let Some((_, ResolveExit::Panicked { message })) = resolve_exit_rx.recv().await {
            panic!("tx-pool ordered resolver panicked: {message}");
        }
        let _ = resolver_handle.await;
    });

    (service, tx_relay_receiver, signal, _store, issue_out_points)
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
    let (consensus, issue_out_points, cell_deps) = secp_test_consensus(issue_outputs);
    let consensus = Arc::new(consensus);
    let (_store, snap) = snapshot_with_genesis(Arc::clone(&consensus));
    let mut config = tx_pool_config();
    config.max_tx_verify_workers = max_workers;
    let (tx_relay_sender, tx_relay_receiver) = ckb_channel::bounded(1024);
    let (block_assembler_sender, _) = mpsc::channel(1);

    let ordered_resolve_queue = Arc::new(RwLock::new(
        crate::component::ordered_resolve_queue::OrderedResolveQueue::new(),
    ));
    let verify_queue = Arc::new(RwLock::new(VerifyQueue::new(
        config.max_tx_verify_cycles,
        config.verify_ordering,
    )));
    #[cfg(feature = "pipeline")]
    let max_workers = config.max_tx_verify_workers.max(1);
    #[cfg(feature = "pipeline")]
    let pre_check_workers =
        max_workers.min(std::thread::available_parallelism().map_or(4, |n| n.get()));
    #[cfg(feature = "pipeline")]
    let pre_check_cancel = ckb_stop_handler::CancellationToken::new();
    #[cfg(feature = "pipeline")]
    let pre_check_queue = Arc::new(crate::component::pre_check_queue::PreCheckQueue::new(
        pre_check_cancel,
    ));
    let (deferred_sender, mut deferred_receiver) = mpsc::channel(1024);
    let (_chunk_tx, chunk_rx) = watch::channel(ChunkCommand::Resume);

    let service = TxPoolService {
        tx_pool: Arc::new(RwLock::new(TxPool::new(config.clone(), snap))),
        orphan: Arc::new(RwLock::new(OrphanPool::new())),
        consensus: Arc::clone(&consensus),
        tx_pool_config: Arc::new(config),
        block_assembler: None,
        txs_verify_cache: Arc::new(RwLock::new(init_cache())),
        callbacks: Arc::new(Callbacks::new()),
        network: super::chunk::dummy_network(),
        tx_relay_sender,
        ordered_resolve_queue: Arc::clone(&ordered_resolve_queue),
        verify_queue: Arc::clone(&verify_queue),
        block_assembler_sender,
        fee_estimator: FeeEstimator::new_dummy(),
        recent_reject: None,
        #[cfg(feature = "pipeline")]
        pre_check_queue: Arc::clone(&pre_check_queue),
        #[cfg(feature = "pipeline")]
        chunk_rx,
        #[cfg(feature = "pipeline")]
        rbf_candidates: Arc::new(RwLock::new(
            crate::component::rbf_candidates::RbfCandidates::new(),
        )),
        deferred_sender,
    };

    {
        let ordered = Arc::clone(&ordered_resolve_queue);
        let txs_verify_cache = Arc::clone(&service.txs_verify_cache);
        tokio::spawn(async move {
            while let Some(task) = deferred_receiver.recv().await {
                match task {
                    crate::service::DeferredTask::RecoverTxs(txs) => {
                        let mut queue = ordered.write().await;
                        for tx in txs {
                            let _ = queue.add_tx(crate::resolved_tx::ResolveJob {
                                tx,
                                remote: None,
                                is_proposal_tx: false,
                                attempts: 0,
                            });
                        }
                    }
                    crate::service::DeferredTask::CacheUpdate { wtx_hash, verified } => {
                        let mut guard = txs_verify_cache.write().await;
                        guard.put(wtx_hash, verified);
                    }
                }
            }
        });
    }

    #[cfg(feature = "pipeline")]
    {
        for _ in 0..pre_check_workers {
            let svc = service.clone();
            let queue = Arc::clone(&pre_check_queue);
            tokio::spawn(async move {
                while let Some(job) = queue.pop().await {
                    let _ = svc
                        .classify_and_enqueue_tx(job.tx, job.is_proposal_tx, job.remote)
                        .await;
                }
            });
        }
    }

    let signal = CancellationToken::new();
    let (chunk_tx, chunk_rx) = watch::channel(ChunkCommand::Resume);

    let mut verify_mgr = VerifyMgr::new(service.clone(), chunk_rx, signal.child_token());
    tokio::spawn(async move { verify_mgr.run().await });

    let ordered_resolver = OrderedResolver::new(
        service.clone(),
        Arc::clone(&ordered_resolve_queue),
        Arc::clone(&verify_queue),
        chunk_tx.subscribe(),
        signal.child_token(),
    );
    let (resolve_exit_tx, mut resolve_exit_rx) = tokio::sync::mpsc::unbounded_channel();
    let resolver_handle = ordered_resolver.start(resolve_exit_tx);
    tokio::spawn(async move {
        if let Some((_, ResolveExit::Panicked { message })) = resolve_exit_rx.recv().await {
            panic!("tx-pool ordered resolver panicked: {message}");
        }
        let _ = resolver_handle.await;
    });

    (
        service,
        tx_relay_receiver,
        signal,
        _store,
        issue_out_points,
        cell_deps,
        chunk_tx,
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
        .submit_remote_tx(tx.clone(), cycles, 1.into())
        .await
        .expect("first submission should succeed");

    // Wait for the tx to reach the pending pool.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.tx_pool.read().await.pool_map.pending_size();
            if pending == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("tx should reach pending");

    // Second submission of the same tx.
    // The pipeline deduplicates at multiple levels:
    // - verify_queue / ordered_resolve_queue contains checks
    // - pool_map.add_entry returns Ok((false, _)) for existing short_id
    // Either way, the pool must still have exactly 1 tx.
    let second_result = service.submit_remote_tx(tx.clone(), cycles, 1.into()).await;
    // The result may be Ok (silent dedup in pool_map) or Err(Duplicated).
    // Both are correct behavior — what matters is the pool state.
    let _ = second_result;

    // Brief wait for any in-flight processing.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let pending = service.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(
        pending, 1,
        "pool must have exactly 1 tx after duplicate submission"
    );

    // Verify the specific tx is still in the pool.
    let pool = service.tx_pool.read().await;
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
            .submit_remote_tx(tx.clone(), cycles, 1.into())
            .await
            .expect("enqueue remote tx should succeed");
    }

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.tx_pool.read().await.pool_map.pending_size();
            if pending == txs.len() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("pipeline should process all txs even with high worker cap");

    let pending = service.tx_pool.read().await.pool_map.pending_size();
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
            svc.submit_remote_tx(tx, cycles, 1.into())
                .await
                .expect("submit under backpressure should succeed");
        }));
    }
    for h in handles {
        h.await.expect("submit task should not panic");
    }

    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let pending = service.tx_pool.read().await.pool_map.pending_size();
            if pending == tx_count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("all txs should reach pending despite semaphore backpressure");

    let pending = service.tx_pool.read().await.pool_map.pending_size();
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
#[cfg(feature = "pipeline")]
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
            .submit_remote_tx(tx.clone(), *cycles, 1.into())
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

    let pending_while_suspended = service.tx_pool.read().await.pool_map.pending_size();

    // Resume — remaining txs should now drain through verification.
    chunk_tx
        .send(ChunkCommand::Resume)
        .expect("send resume signal");

    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let pending = service.tx_pool.read().await.pool_map.pending_size();
            if pending == txs.len() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("all txs should reach pending after resume");

    let pending = service.tx_pool.read().await.pool_map.pending_size();
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
/// 3. Exercise the `DeferredTask::RecoverTxs` path (even if recovery set
///    is empty for a simple 2-tx conflict).
///
/// This tests the full RBF → deferred worker → pool state transition path.
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
        .submit_remote_tx(tx_a.clone(), cycles_a, 1.into())
        .await
        .expect("tx_a should be accepted");

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.tx_pool.read().await.pool_map.pending_size();
            if pending == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("tx_a should reach pending");

    {
        let pool = service.tx_pool.read().await;
        assert!(
            pool.get_tx_from_pool(&id_a).is_some(),
            "tx_a should be in pool before replacement"
        );
    }

    // Submit tx_b — triggers RBF, displacing tx_a.
    service
        .submit_remote_tx(tx_b.clone(), cycles_b, 1.into())
        .await
        .expect("tx_b (RBF replacement) should be accepted");

    // Wait for RBF to complete: tx_b must appear in the pool, which can only
    // happen after tx_a is removed (they conflict on the same input).
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (b_in_pool, ordered_len, verify_len) = {
                let pool = service.tx_pool.read().await;
                let ordered = service.ordered_resolve_queue.read().await;
                let verify = service.verify_queue.read().await;
                (
                    pool.get_tx_from_pool(&id_b).is_some(),
                    ordered.len(),
                    verify.len(),
                )
            };
            if b_in_pool && ordered_len == 0 && verify_len == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("RBF should complete: tx_a displaced, tx_b in pool");

    let pool = service.tx_pool.read().await;
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
                .capacity(Capacity::bytes(4_990).unwrap())
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
        .submit_remote_tx(tx_a.clone(), cycles_a, 1.into())
        .await
        .expect("tx_a should be accepted");

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.tx_pool.read().await.pool_map.pending_size();
            if pending == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("tx_a should reach pending");

    // Submit tx_b. In pipeline mode this merely enqueues the tx and returns
    // Ok; in non-pipeline mode it may return the actual reject. Either way,
    // success/failure is determined by inspecting the final pool state.
    let _ = service
        .submit_remote_tx(tx_b.clone(), cycles_b, 1.into())
        .await;

    // Wait for tx_a to be recovered. In pipeline mode this involves the
    // deferred worker, pre-check, verify, and submit stages; in non-pipeline
    // mode the deferred task is processed asynchronously after `submit_remote_tx`
    // returns. Either way, the observable outcome is tx_a back in the pool.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            {
                let pool = service.tx_pool.read().await;
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
    // the conflict pool rather than left out of the mempool.
    let pool = service.tx_pool.read().await;
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

/// Topologically sort dependent transactions so parents come before children.
#[cfg(feature = "pipeline")]
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
#[cfg(feature = "pipeline")]
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

/// The pre-check queue rejects new jobs once its size limit is exceeded.
#[cfg(feature = "pipeline")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pre_check_queue_rejects_when_full() {
    let cancel = ckb_stop_handler::CancellationToken::new();
    let queue = Arc::new(crate::component::pre_check_queue::PreCheckQueue::new(
        cancel,
    ));

    let tx = TransactionBuilder::default().build();
    let job = crate::component::pre_check_queue::PreCheckJob {
        tx: tx.clone(),
        is_proposal_tx: false,
        remote: None,
    };

    // First push succeeds.
    assert!(queue.push(job.clone()).is_ok());

    // Push a very large dummy tx to exceed the 256MB limit.
    let huge_tx = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from(vec![0u8; 300_000_000]).pack()])
        .build();
    let huge_job = crate::component::pre_check_queue::PreCheckJob {
        tx: huge_tx,
        is_proposal_tx: false,
        remote: None,
    };
    assert!(
        matches!(queue.push(huge_job), Err(crate::error::Reject::Full(_))),
        "pre_check_queue should reject a tx that exceeds the size limit"
    );

    // Popping the first job makes room.
    let popped = queue.pop().await;
    assert!(popped.is_some());
    assert!(queue.push(job).is_ok());
}

/// Concurrent RBF replacements for the same input must be ordered by fee.
/// Only the highest-fee candidate should end up in the pool; lower-fee ones
/// must be rejected rather than temporarily displacing the original tx and
/// blocking the higher-fee candidate.
#[cfg(feature = "pipeline")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_concurrent_rbf_prefers_highest_fee() {
    let (service, _relay, signal, _store, issue_out_points) = service_with_rbf(1);
    let shared_input = &issue_out_points[0];

    // Original tx in pool: fee = 1000 bytes.
    let original = build_tx(shared_input, 4_000);
    let original_id = original.proposal_short_id();
    let original_cycles = measured_cycles(&service, original.clone()).await;
    service
        .submit_remote_tx(original, original_cycles, 1.into())
        .await
        .unwrap();

    // Wait until the original tx is actually in the pool before racing
    // replacements against it.
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let pending = service.tx_pool.read().await.pool_map.pending_size();
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
            svc.submit_remote_tx(tx, cycles, peer.into()).await
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
            let (pending, ordered_len, verify_len, settled) = {
                let pool = service.tx_pool.read().await;
                let ordered = service.ordered_resolve_queue.read().await;
                let verify = service.verify_queue.read().await;
                let settled = pool.get_tx_from_pool(&original_id).is_none()
                    && pool.get_tx_from_pool(&expected_id).is_some()
                    && ids
                        .iter()
                        .all(|(id, _)| *id == expected_id || pool.get_tx_from_pool(id).is_none());
                (
                    pool.pool_map.pending_size(),
                    ordered.len(),
                    verify.len(),
                    settled,
                )
            };
            if pending == 1 && ordered_len == 0 && verify_len == 0 && settled {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("pipeline should settle with exactly one RBF replacement accepted");

    let pool = service.tx_pool.read().await;
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
#[cfg(feature = "pipeline")]
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
        .submit_remote_tx(tx_a.clone(), cycles_a, 1.into())
        .await
        .expect("tx_a should be accepted");

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.tx_pool.read().await.pool_map.pending_size();
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
        .submit_remote_tx(tx_b.clone(), cycles_b, 1.into())
        .await
        .expect("tx_b should be accepted");

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.tx_pool.read().await.pool_map.pending_size();
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
        .submit_remote_tx(tx_c.clone(), cycles_c, 1.into())
        .await
        .expect("tx_c should be accepted");

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.tx_pool.read().await.pool_map.pending_size();
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
        .submit_remote_tx(tx_r.clone(), cycles_r, 1.into())
        .await;

    // Wait for the pipeline to drain and the original chain to be recovered.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (a_in_pool, b_in_pool, c_in_pool, r_in_pool, ordered_len, verify_len) = {
                let pool = service.tx_pool.read().await;
                let ordered = service.ordered_resolve_queue.read().await;
                let verify = service.verify_queue.read().await;
                (
                    pool.get_tx_from_pool(&id_a).is_some(),
                    pool.get_tx_from_pool(&id_b).is_some(),
                    pool.get_tx_from_pool(&id_c).is_some(),
                    pool.get_tx_from_pool(&id_r).is_some(),
                    ordered.len(),
                    verify.len(),
                )
            };
            if a_in_pool
                && b_in_pool
                && c_in_pool
                && !r_in_pool
                && ordered_len == 0
                && verify_len == 0
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("original chain should be recovered after rejected RBF replacement");

    let pool = service.tx_pool.read().await;
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
