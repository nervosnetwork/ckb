//! End-to-end tests for the tx-pool resolve -> verify -> submit pipeline.

use crate::callback::Callbacks;
use crate::component::orphan::OrphanPool;
use crate::component::resolve_queue::ResolveQueue;
use crate::component::verify_queue::VerifyQueue;
use crate::pool::TxPool;
use crate::resolve_mgr::{OrderedResolver, PreResolveMgr, ResolveExit};
use crate::service::TxPoolService;
use crate::verify_mgr::VerifyMgr;
use ckb_app_config::TxPoolConfig;
use ckb_chain_spec::consensus::Consensus;
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_fee_estimator::FeeEstimator;
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_stop_handler::CancellationToken;
use ckb_store::attach_block_cell;
use ckb_crypto::secp::Privkey;
use ckb_system_scripts::BUNDLED_CELL;
use ckb_test_chain_utils::{MockStore, always_success_cell};
use ckb_types::{
    H160, H256, U256,
    bytes::Bytes,
    core::{
        BlockBuilder, BlockExt, Capacity, EpochNumberWithFraction, TransactionBuilder,
        TransactionView,
    },
    h160, h256,
    packed::{CellDep, CellInput, CellOutput, OutPoint, Script, WitnessArgs},
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

    let resolve_queue = Arc::new(RwLock::new(ResolveQueue::new()));
    let ordered_resolve_queue = Arc::new(RwLock::new(
        crate::component::ordered_resolve_queue::OrderedResolveQueue::new(),
    ));
    let verify_queue = Arc::new(RwLock::new(VerifyQueue::new(config.max_tx_verify_cycles)));

    let service = TxPoolService {
        tx_pool: Arc::new(RwLock::new(TxPool::new(config.clone(), snap))),
        orphan: Arc::new(RwLock::new(OrphanPool::new())),
        consensus: Arc::clone(&consensus),
        tx_pool_config: Arc::new(config.clone()),
        block_assembler: None,
        txs_verify_cache: Arc::new(RwLock::new(init_cache())),
        callbacks: Arc::new(Callbacks::new()),
        network: super::chunk::network(&consensus),
        tx_relay_sender,
        resolve_queue: Arc::clone(&resolve_queue),
        ordered_resolve_queue: Arc::clone(&ordered_resolve_queue),
        verify_queue: Arc::clone(&verify_queue),
        block_assembler_sender,
        fee_estimator: FeeEstimator::new_dummy(),
    };

    let signal = CancellationToken::new();
    let (chunk_tx, chunk_rx) = watch::channel(ChunkCommand::Resume);

    let mut verify_mgr = VerifyMgr::new(service.clone(), chunk_rx, signal.child_token());
    tokio::spawn(async move { verify_mgr.run().await });

    let mut pre_resolve_mgr = PreResolveMgr::new(
        service.clone(),
        Arc::clone(&resolve_queue),
        Arc::clone(&ordered_resolve_queue),
        Arc::clone(&verify_queue),
        chunk_tx.subscribe(),
        signal.child_token(),
    );
    tokio::spawn(async move { pre_resolve_mgr.run().await });

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
        if let Some(ResolveExit::Panicked { message }) = resolve_exit_rx.recv().await {
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
        ._test_accept_tx(tx)
        .await
        .expect("local test accept should succeed")
        .cycles
}

// -----------------------------------------------------------------------------
// secp256k1 1-in-1-out helpers
// -----------------------------------------------------------------------------

const SECP_PRIVKEY: H256 = h256!("0xb2b3324cece882bca684eaf202667bb56ed8e8c2fd4b4dc71f615ebd6d9055a5");
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

    let resolve_queue = Arc::new(RwLock::new(ResolveQueue::new()));
    let ordered_resolve_queue = Arc::new(RwLock::new(
        crate::component::ordered_resolve_queue::OrderedResolveQueue::new(),
    ));
    let verify_queue = Arc::new(RwLock::new(VerifyQueue::new(config.max_tx_verify_cycles)));

    let service = TxPoolService {
        tx_pool: Arc::new(RwLock::new(TxPool::new(config.clone(), snap))),
        orphan: Arc::new(RwLock::new(OrphanPool::new())),
        consensus: Arc::clone(&consensus),
        tx_pool_config: Arc::new(config.clone()),
        block_assembler: None,
        txs_verify_cache: Arc::new(RwLock::new(init_cache())),
        callbacks: Arc::new(Callbacks::new()),
        network: super::chunk::network(&consensus),
        tx_relay_sender,
        resolve_queue: Arc::clone(&resolve_queue),
        ordered_resolve_queue: Arc::clone(&ordered_resolve_queue),
        verify_queue: Arc::clone(&verify_queue),
        block_assembler_sender,
        fee_estimator: FeeEstimator::new_dummy(),
    };

    let signal = CancellationToken::new();
    let (chunk_tx, chunk_rx) = watch::channel(ChunkCommand::Resume);

    let mut verify_mgr = VerifyMgr::new(service.clone(), chunk_rx, signal.child_token());
    tokio::spawn(async move { verify_mgr.run().await });

    let mut pre_resolve_mgr = PreResolveMgr::new(
        service.clone(),
        Arc::clone(&resolve_queue),
        Arc::clone(&ordered_resolve_queue),
        Arc::clone(&verify_queue),
        chunk_tx.subscribe(),
        signal.child_token(),
    );
    tokio::spawn(async move { pre_resolve_mgr.run().await });

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
        if let Some(ResolveExit::Panicked { message }) = resolve_exit_rx.recv().await {
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
        .lock(lock.clone())
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
        ._process_tx(tx, None, None)
        .await
        .expect("local process tx should return a result");
    ret.expect("local tx should be accepted").cycles
}

async fn clear_verify_cache(service: &TxPoolService) {
    let mut cache = service.txs_verify_cache.write().await;
    *cache = init_cache();
}

async fn verify_cycles(service: &TxPoolService, tx: TransactionView) -> u64 {
    let (pre_check_ret, snapshot) = service.pre_check(&tx).await;
    let (_tip_hash, rtx, status, _fee, _tx_size) =
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

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn pipeline_throughput() {
    const TX_COUNT: usize = 500;
    let (service, _relay, signal, _store, issue_out_points) =
        service_with_pipeline_workers(TX_COUNT, 8);

    let txs: Vec<_> = issue_out_points
        .iter()
        .map(|out_point| build_tx(out_point, 4_000))
        .collect();

    let cycles = measured_cycles(&service, txs[0].clone()).await;

    let start = std::time::Instant::now();

    let handles: Vec<_> = txs
        .into_iter()
        .map(|tx| {
            let service = service.clone();
            tokio::spawn(async move {
                service
                    .submit_remote_tx(tx, cycles, 1.into())
                    .await
                    .expect("enqueue remote tx should succeed");
            })
        })
        .collect();
    for h in handles {
        h.await.unwrap();
    }

    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let pending = service.tx_pool.read().await.pool_map.pending_size();
            if pending == TX_COUNT {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("pipeline should process all txs in time");

    let elapsed = start.elapsed();
    let throughput = TX_COUNT as f64 / elapsed.as_secs_f64();
    eprintln!("pipeline throughput: {TX_COUNT} txs in {elapsed:?} => {throughput:.1} tx/s",);

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
    let handle_a = tokio::spawn(async move {
        service_a
            .submit_remote_tx(tx_a, cycles_a, 1.into())
            .await
    });
    let handle_b = tokio::spawn(async move {
        service_b
            .submit_remote_tx(tx_b, cycles_b, 1.into())
            .await
    });

    let (res_a, res_b) = tokio::join!(handle_a, handle_b);
    let _ = res_a.expect("task a should not panic");
    let _ = res_b.expect("task b should not panic");

    // Wait for the pipeline to drain. Both txs should leave the resolve/verify
    // queues and exactly one must land in the pending pool.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (pending, resolve_len, verify_len) = {
                let pool = service.tx_pool.read().await;
                let resolve = service.resolve_queue.read().await;
                let verify = service.verify_queue.read().await;
                (
                    pool.pool_map.pending_size(),
                    resolve.len(),
                    verify.len(),
                )
            };
            if pending == 1 && resolve_len == 0 && verify_len == 0 {
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
    let child = build_secp_tx(&parent_output, &cell_deps, SECP_ISSUE_CAPACITY - 2 * SECP_FEE);

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

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn secp_remote_throughput() {
    // Smaller count than the always-success benchmark because real secp256k1
    // verification is much heavier, especially in debug builds.
    const TX_COUNT: usize = 500;
    let (service, _relay, signal, _store, issue_out_points, cell_deps) =
        secp_service_with_pipeline_workers(TX_COUNT, 8);

    let txs: Vec<_> = issue_out_points
        .iter()
        .map(|out_point| build_secp_tx(out_point, &cell_deps, SECP_ISSUE_CAPACITY - SECP_FEE))
        .collect();

    // Measure exact cycles for every tx without adding them to the pool, so the
    // benchmark only times the pipeline (resolve + verify + submit).
    let mut cycles = Vec::with_capacity(txs.len());
    for tx in &txs {
        cycles.push(verify_cycles(&service, tx.clone()).await);
    }

    // Clear the verify cache so the pipeline has to run real secp256k1 script
    // verification, matching what happens when txs first arrive from the network.
    clear_verify_cache(&service).await;

    let start = std::time::Instant::now();

    let handles: Vec<_> = txs
        .into_iter()
        .zip(cycles)
        .map(|(tx, cycles)| {
            let service = service.clone();
            tokio::spawn(async move {
                service
                    .submit_remote_tx(tx, cycles, 1.into())
                    .await
                    .expect("enqueue secp remote tx should succeed");
            })
        })
        .collect();
    for h in handles {
        h.await.unwrap();
    }

    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let pending = service.tx_pool.read().await.pool_map.pending_size();
            if pending == TX_COUNT {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("pipeline should process all secp txs in time");

    let elapsed = start.elapsed();
    let throughput = TX_COUNT as f64 / elapsed.as_secs_f64();
    let mode = if cfg!(feature = "pipeline") {
        "pipeline"
    } else {
        "sync"
    };
    eprintln!(
        "{mode} secp throughput: {TX_COUNT} txs in {elapsed:?} => {throughput:.1} tx/s",
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}
