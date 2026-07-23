//! Criterion benchmark for the tx-pool resolve -> verify -> submit pipeline.
//!
//! This module is only available with the `internal` feature. The actual binary
//! target lives in `tx-pool/benches/pipeline.rs`.
//!
//! Run with Criterion's built-in baseline facility:
//!
//! ```sh
//! cargo bench -p ckb-tx-pool --features internal --bench pipeline -- --save-baseline current
//! ```

use crate::network::{DummyTxPoolNetwork, TxPoolNetworkHandle};
use crate::resolve_mgr::{OrderedResolver, ResolveExit};
use crate::service::TxPoolService;
use ckb_app_config::{TxPoolConfig, VerifyOrdering};
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_crypto::secp::Privkey;
use ckb_dao_utils::genesis_dao_data;
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
        BlockBuilder, BlockExt, Capacity, EpochNumberWithFraction, FeeRate, TransactionBuilder,
        TransactionView,
    },
    h160, h256,
    packed::{CellDep, CellInput, CellOutput, OutPoint, Script, WitnessArgs},
    prelude::*,
    utilities::difficulty_to_compact,
};
use ckb_verification::cache::init_cache;
use criterion::{BatchSize, BenchmarkGroup, Criterion, SamplingMode, Throughput, criterion_group};
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, RwLock, watch};

const MAX_TX_VERIFY_CYCLES: u64 = 70_000_000;
const ALWAYS_SUCCESS_ISSUE_CAPACITY: u64 = 5_000;

const SECP_PRIVKEY: H256 =
    h256!("0xb2b3324cece882bca684eaf202667bb56ed8e8c2fd4b4dc71f615ebd6d9055a5");
const SECP_PUBKEY_HASH: H160 = h160!("0x779e5930892a0a9bf2fedfe048f685466c7d0396");
const SECP_ISSUE_CAPACITY: u64 = 50_000 * 100_000_000;
const SECP_FEE: u64 = 1_000 * 100_000_000;

// ---------------------------------------------------------------------------
// Config helpers
// ---------------------------------------------------------------------------

fn tx_pool_config(max_workers: usize) -> TxPoolConfig {
    TxPoolConfig {
        max_tx_pool_size: 180_000_000,
        min_fee_rate: FeeRate::zero(),
        min_rbf_rate: FeeRate::zero(),
        max_tx_verify_cycles: MAX_TX_VERIFY_CYCLES,
        max_tx_verify_workers: max_workers,
        max_ancestors_count: 1000,
        keep_rejected_tx_hashes_days: 1,
        keep_rejected_tx_hashes_count: 1000,
        persisted_data: Default::default(),
        recent_reject: Default::default(),
        expiry_hours: 24,
        verify_ordering: VerifyOrdering::ArrivalTime,
        max_verify_queue_tx_size: 256_000_000,
    }
}

// ---------------------------------------------------------------------------
// Chain / script helpers
// ---------------------------------------------------------------------------

fn always_success_script() -> Script {
    always_success_cell().2.clone()
}

fn always_success_dep() -> CellDep {
    CellDep::new_builder()
        .out_point(ckb_test_chain_utils::create_always_success_out_point())
        .build()
}

fn test_consensus(issue_outputs: usize) -> (Consensus, TransactionView) {
    let (always_success_cell, always_success_data, always_success_script) = always_success_cell();

    let always_success_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .output_data(always_success_data)
        .witness(always_success_script.clone().into_witness())
        .build();

    let issue_output = CellOutput::new_builder()
        .capacity(Capacity::bytes(ALWAYS_SUCCESS_ISSUE_CAPACITY as usize).unwrap())
        .lock(always_success_script.clone())
        .build();
    let issue_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .outputs((0..issue_outputs).map(|_| issue_output.clone()))
        .outputs_data((0..issue_outputs).map(|_| Bytes::default().pack()))
        .build();

    let dao = genesis_dao_data(vec![&always_success_tx, &issue_tx]).unwrap();
    let genesis = BlockBuilder::default()
        .timestamp(1_557_310_743u64)
        .compact_target(difficulty_to_compact(U256::from(1000u64)))
        .dao(dao)
        .transaction(always_success_tx)
        .transaction(issue_tx.clone())
        .build();

    let consensus = ConsensusBuilder::default()
        .genesis_block(genesis)
        .cellbase_maturity(EpochNumberWithFraction::new(0, 0, 1))
        .build();

    (consensus, issue_tx)
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

// ---------------------------------------------------------------------------
// SharedBench — resources shared across all benchmark iterations
// ---------------------------------------------------------------------------

struct SharedBench {
    _store: MockStore,
    issue_tx: TransactionView,
    snapshot: Arc<Snapshot>,
    network: TxPoolNetworkHandle,
    runtime: tokio::runtime::Runtime,
    ckb_handle: ckb_async_runtime::Handle,
    secp_cell_deps: Option<Vec<CellDep>>,
}

/// Event-driven stable-state completion barrier for measured submissions.
///
/// Polling the controller every millisecond adds timer quantization and
/// dispatcher contention to cases that complete in only a few milliseconds.
/// The pending callback runs after the authoritative pool mutation, so it is
/// the precise completion event the benchmark needs.
#[derive(Default)]
struct BenchCompletion {
    completed: AtomicUsize,
    changed: Notify,
}

impl BenchCompletion {
    fn record(&self) {
        self.completed.fetch_add(1, Ordering::Release);
        // `notify_one` stores one permit when no waiter is currently polled,
        // closing the load-to-await race. Coalescing is harmless because the
        // atomic count is the source of truth.
        self.changed.notify_one();
    }

    async fn wait_for(&self, target: usize) {
        let result = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                if self.completed.load(Ordering::Acquire) >= target {
                    break;
                }
                self.changed.notified().await;
            }
        })
        .await;
        assert!(
            result.is_ok(),
            "pipeline did not complete all txs in time: {}/{} accepted",
            self.completed.load(Ordering::Acquire),
            target
        );
    }
}

impl SharedBench {
    fn from_consensus(
        consensus: Consensus,
        issue_tx: TransactionView,
        secp_cell_deps: Option<Vec<CellDep>>,
    ) -> Self {
        let consensus = Arc::new(consensus);
        let (store, snapshot) = snapshot_with_genesis(Arc::clone(&consensus));
        let network: TxPoolNetworkHandle = Arc::new(DummyTxPoolNetwork);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(8)
            .enable_all()
            .build()
            .expect("build tokio runtime");
        let ckb_handle = ckb_async_runtime::Handle::new(runtime.handle().clone(), None);
        Self {
            _store: store,
            issue_tx,
            snapshot,
            network,
            runtime,
            ckb_handle,
            secp_cell_deps,
        }
    }

    fn new_always_success(issue_outputs: usize) -> Self {
        let (consensus, issue_tx) = test_consensus(issue_outputs);
        Self::from_consensus(consensus, issue_tx, None)
    }

    fn new_secp(issue_outputs: usize) -> (Self, Vec<CellDep>) {
        let (consensus, issue_tx, _, cell_deps) = secp_test_consensus(issue_outputs);
        (
            Self::from_consensus(consensus, issue_tx, Some(cell_deps.clone())),
            cell_deps,
        )
    }

    fn issue_out_points(&self, count: usize) -> Vec<OutPoint> {
        (0..count)
            .map(|i| OutPoint::new(self.issue_tx.hash(), i as u32))
            .collect()
    }

    /// Start a full tx-pool service (with controller) for benchmark iterations.
    ///
    /// The returned [`ServiceHandle`] owns a clone of `tx_relay_sender` so that
    /// dropping it closes the relay channel. It also owns the production main
    /// dispatcher handle; awaiting both handles on drop proves every message
    /// handler and worker has quiesced before the next iteration begins.
    fn start_controller(&self, max_workers: usize) -> ServiceHandle {
        let config = tx_pool_config(max_workers);
        let (tx_relay_sender, tx_relay_receiver) = ckb_channel::bounded(1024);

        // Drain relay results so the channel behaves like a real relayer consumer.
        let drain_handle = self
            .runtime
            .spawn_blocking(move || while tx_relay_receiver.recv().is_ok() {});

        // Each controller gets a fresh, empty verify cache. init_cache() creates
        // a new LruCache with no global state, so cycle-measurement results from
        // BenchData::new() cannot leak into benchmark iterations.
        let (mut builder, controller) = crate::TxPoolServiceBuilder::new(
            config,
            Arc::clone(&self.snapshot),
            None,
            Arc::new(RwLock::new(init_cache())),
            &self.ckb_handle,
            tx_relay_sender.clone(),
            FeeEstimator::new_dummy(),
        );
        let completion = Arc::new(BenchCompletion::default());
        let completion_callback = Arc::clone(&completion);
        builder.register_pending(Box::new(move |_| completion_callback.record()));
        // Replace the global exit token with a local one so stopping this service
        // does not affect other benchmark iterations.
        let local_signal = CancellationToken::new();
        builder.signal_receiver = local_signal.clone();
        let dispatcher_handle = builder.start_with_handle(Arc::clone(&self.network));
        let service = ServiceHandle {
            controller,
            signal: local_signal,
            tx_relay_sender: Some(tx_relay_sender),
            drain_handle: Some(drain_handle),
            dispatcher_handle: Some(dispatcher_handle),
            runtime: self.ckb_handle.clone(),
            completion,
        };

        // `start_inner` has spawned every worker, but Tokio is free to run the
        // benchmark thread before those tasks receive their first poll. Prove
        // the dispatcher is live, then yield one short scheduler interval so
        // worker-start latency is not charged to the first transaction batch.
        // This is setup work outside Criterion's measured closure.
        service
            .controller
            .get_tx_pool_info()
            .expect("benchmark dispatcher readiness round-trip");
        self.runtime.block_on(async {
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(1)).await;
        });
        service
    }
}

/// Await a set of tokio task handles with a timeout, running the wait on a
/// background thread so this can be called safely from both sync and async
/// contexts.
fn await_handles(
    runtime: &ckb_async_runtime::Handle,
    handles: Vec<tokio::task::JoinHandle<()>>,
    timeout: Duration,
) {
    if handles.is_empty() {
        return;
    }
    let runtime = runtime.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = runtime.block_on(async move {
            match tokio::time::timeout(timeout, futures_util::future::join_all(handles)).await {
                Ok(results) => {
                    let failures: Vec<_> = results
                        .into_iter()
                        .filter_map(Result::err)
                        .map(|error| error.to_string())
                        .collect();
                    if failures.is_empty() {
                        Ok(())
                    } else {
                        Err(format!(
                            "benchmark teardown task failed: {}",
                            failures.join("; ")
                        ))
                    }
                }
                Err(_) => Err(format!(
                    "benchmark teardown did not quiesce within {timeout:?}"
                )),
            }
        });
        let _ = tx.send(result);
    });
    match rx.recv_timeout(timeout + Duration::from_secs(1)) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("{error}"),
        Err(error) => panic!("benchmark teardown helper did not complete: {error}"),
    }
}

/// RAII handle for a benchmark service instance.
///
/// On drop it cancels the [`CancellationToken`] (shutting down all spawned
/// workers), drops the `tx_relay_sender` clone so the relay channel closes,
/// and awaits the dispatcher plus drain handles so no worker or blocking
/// thread overlaps the next iteration.
struct ServiceHandle {
    controller: crate::TxPoolController,
    signal: CancellationToken,
    /// Stored so we can explicitly drop it on shutdown, closing the relay
    /// channel and letting the background drain task exit.
    tx_relay_sender: Option<ckb_channel::Sender<crate::service::TxVerificationResult>>,
    /// Handle for the blocking relay-drain task.
    drain_handle: Option<tokio::task::JoinHandle<()>>,
    /// Main dispatcher owns and quiesces every production worker handle.
    dispatcher_handle: Option<tokio::task::JoinHandle<()>>,
    /// Runtime used to await service teardown on drop.
    runtime: ckb_async_runtime::Handle,
    /// Stable-state completion event used instead of controller polling.
    completion: Arc<BenchCompletion>,
}

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        self.signal.cancel();
        // Drop the sender explicitly so the relay drain task (spawn_blocking)
        // sees a closed channel and exits instead of leaking.
        self.tx_relay_sender.take();
        let mut handles = Vec::with_capacity(2);
        if let Some(handle) = self.dispatcher_handle.take() {
            handles.push(handle);
        }
        if let Some(handle) = self.drain_handle.take() {
            handles.push(handle);
        }
        await_handles(
            &self.runtime,
            handles,
            Duration::from_secs(crate::constants::PIPELINE_SHUTDOWN_TIMEOUT_SECONDS + 5),
        );
    }
}

/// RAII handle for the bare service used by cycle measurement.
struct BenchServiceHandle {
    /// Kept alive so that the underlying service is not dropped while
    /// benchmark tasks are still running.
    service: Option<TxPoolService>,
    signal: CancellationToken,
    worker_handles: Vec<tokio::task::JoinHandle<()>>,
    effect_publisher: Option<tokio::task::JoinHandle<()>>,
    drain_handle: Option<tokio::task::JoinHandle<()>>,
    runtime: ckb_async_runtime::Handle,
}

impl BenchServiceHandle {
    fn service(&self) -> &TxPoolService {
        self.service
            .as_ref()
            .expect("benchmark service is unavailable during teardown")
    }
}

impl Drop for BenchServiceHandle {
    fn drop(&mut self) {
        self.signal.cancel();
        // Worker tasks own service clones, so join them first. Then release the
        // handle's final service (and relay sender) before awaiting the relay
        // drain; doing these in one join set creates a sender/drain deadlock.
        let worker_handles = std::mem::take(&mut self.worker_handles);
        await_handles(&self.runtime, worker_handles, Duration::from_secs(5));
        if let Some(service) = &self.service {
            service.relay.effects.close();
        }
        if let Some(effect_publisher) = self.effect_publisher.take() {
            await_handles(
                &self.runtime,
                vec![effect_publisher],
                Duration::from_secs(5),
            );
        }
        self.service.take();
        if let Some(drain_handle) = self.drain_handle.take() {
            await_handles(&self.runtime, vec![drain_handle], Duration::from_secs(5));
        }
    }
}

// ---------------------------------------------------------------------------
// Async helpers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// start_service — used by measure_cycles (needs direct TxPoolService access)
// ---------------------------------------------------------------------------

/// Build a [`TxPoolService`] together with its pipeline workers for direct
/// method calls (e.g. `process_tx`, `test_accept_tx`).
///
/// This uses [`TxPoolServiceBuilder::build_bench_service`] for the service
/// construction, ensuring the same assembly path as production code.  The
/// returned [`BenchServiceHandle`] owns all spawned task handles and awaits
/// their clean shutdown on drop.
fn start_service(shared: &SharedBench, max_workers: usize) -> BenchServiceHandle {
    let config = tx_pool_config(max_workers);
    let (tx_relay_sender, tx_relay_receiver) = ckb_channel::bounded(1024);
    let drain_handle = shared
        .runtime
        .spawn_blocking(move || while tx_relay_receiver.recv().is_ok() {});

    let (mut builder, _controller) = crate::TxPoolServiceBuilder::new(
        config,
        Arc::clone(&shared.snapshot),
        None,
        Arc::new(RwLock::new(init_cache())),
        &shared.ckb_handle,
        tx_relay_sender,
        FeeEstimator::new_dummy(),
    );
    // Use a fresh exit token for the bench service so the pre-check workers are
    // not affected by the process-wide exit signal used by the builder.
    let local_signal = CancellationToken::new();
    builder.signal_receiver = local_signal;
    let mut parts = builder.build_bench_service(Arc::clone(&shared.network));

    let mut worker_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    // Spawn the best-effort verification-cache update worker. Holding a full
    // TxPoolService here would keep its channel sender open forever.
    {
        let handle = ckb_async_runtime::Handle::new(tokio::runtime::Handle::current(), None);
        worker_handles.push(crate::service::spawn_verify_cache_worker(
            &handle,
            Arc::clone(&parts.service.aux.txs_verify_cache),
            parts.verify_cache_receiver,
            parts.signal.child_token(),
        ));
    }

    // Spawn pipeline workers using the components from the builder.
    let signal = parts.signal;

    {
        let pre_check_workers =
            max_workers.min(std::thread::available_parallelism().map_or(4, |n| n.get()));
        for _ in 0..pre_check_workers {
            let svc = parts.service.clone();
            worker_handles.push(tokio::spawn(
                crate::service::workers::run_pre_check_worker_loop(svc),
            ));
        }
    }

    // Create a fresh chunk channel shared by VerifyMgr, OrderedResolver and the
    // service itself (the reorg recovery path needs to observe pause/resume).
    let (chunk_tx, chunk_rx) = watch::channel(ChunkCommand::Resume);
    parts.service.pipeline.chunk_rx = chunk_rx.clone();

    let mut verify_mgr =
        crate::verify_mgr::VerifyMgr::new(parts.service.clone(), chunk_rx, signal.child_token());
    worker_handles.push(tokio::spawn(async move { verify_mgr.run().await }));

    let ordered_resolver = OrderedResolver::new(
        parts.service.clone(),
        chunk_tx.subscribe(),
        signal.child_token(),
    );
    let (resolve_exit_tx, mut resolve_exit_rx) = tokio::sync::mpsc::unbounded_channel();
    let resolver_handle = ordered_resolver.start(resolve_exit_tx);
    worker_handles.push(tokio::spawn(async move {
        if let Some((_, ResolveExit::Panicked { message })) = resolve_exit_rx.recv().await {
            panic!("tx-pool ordered resolver panicked: {message}");
        }
        let _ = resolver_handle.await;
    }));

    BenchServiceHandle {
        service: Some(parts.service),
        signal,
        worker_handles,
        effect_publisher: Some(parts.effect_publisher),
        drain_handle: Some(drain_handle),
        runtime: shared.ckb_handle.clone(),
    }
}

// ---------------------------------------------------------------------------
// Cycle measurement
// ---------------------------------------------------------------------------

/// How cycle counts should be measured for a set of transactions.
enum MeasureMode {
    /// Measure one sample tx via `test_accept_tx` and apply the same cycle
    /// count to every transaction in the set (appropriate for always_success
    /// scripts whose verification cost is deterministic).
    Uniform,
    /// Measure each tx individually via `process_tx`.  Required for dependent
    /// chains whose inputs resolve through the pool's locked path.
    PerTxProcess,
    /// Measure each tx individually via `test_accept_tx`.  Required for
    /// independent secp256k1 txs whose ECDSA verification cycles vary
    /// non-deterministically with signature values.
    PerTxTest,
}

/// Poll the tx-pool service until at least `count` transactions reach the
/// pending state.
///
/// This is the service-level counterpart of [`wait_for_pending`] and is used
/// when no controller exists (e.g. during cycle measurement).
async fn wait_for_pending_service(service: &TxPoolService, count: usize) {
    let start = Instant::now();
    let mut last_log = start;
    let result = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending >= count {
                break;
            }
            let now = Instant::now();
            if now.duration_since(last_log).as_secs() >= 5 {
                eprintln!(
                    "[wait_for_pending_service] pending={} after {:?}",
                    pending,
                    now.duration_since(start)
                );
                last_log = now;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert!(
        result.is_ok(),
        "pipeline did not process all txs in time: {}/{} pending",
        pending,
        count
    );
}

/// Measure verification cycles for a set of transactions.
///
/// For dependent chains, txs are submitted through the real pipeline via
/// `notify_tx` (which does not require declared cycles), so the cycle numbers
/// reflect the full resolve -> verify -> submit path including orphan recovery
/// and the ordered resolver.  Children are submitted before their parents so
/// the measurement exercises the recovery path.
///
/// secp256k1 ECDSA verification has non-deterministic cycle counts per tx
/// (different signature values take different CKB-VM execution paths), so each
/// tx must be measured individually — we cannot just measure one and multiply.
fn measure_cycles(
    shared: &SharedBench,
    txs: &[TransactionView],
    mode: MeasureMode,
) -> HashMap<ckb_types::packed::Byte32, u64> {
    let handle = shared.runtime.block_on(async { start_service(shared, 8) });
    let cycles = shared.runtime.block_on(async {
        let mut cycles = HashMap::with_capacity(txs.len());
        match mode {
            MeasureMode::Uniform => {
                let sample = &txs[0];
                let c = handle
                    .service()
                    .test_accept_tx(sample.clone())
                    .await
                    .expect("measure uniform cycle")
                    .cycles;
                for tx in txs {
                    cycles.insert(tx.hash(), c);
                }
            }
            MeasureMode::PerTxProcess => {
                // Submit dependent chains one tx at a time, waiting for each to
                // reach the pending pool before submitting the next.  This
                // avoids the orphan-recovery race that occurs when a long chain
                // is submitted all at once, while still measuring cycles through
                // the real resolve -> verify -> submit pipeline.
                for tx in txs {
                    // Capture the target before enqueueing. `notify_tx` only
                    // guarantees dispatch, and a fast worker may commit before
                    // it returns; deriving `pending + 1` afterwards can wait for
                    // a transaction that was never submitted.
                    let expected_pending = {
                        let pool = handle.service().pool.tx_pool.read().await;
                        pool.pool_map.pending_size() + 1
                    };
                    handle
                        .service()
                        .notify_tx(tx.clone())
                        .await
                        .expect("measure cycles via notify_tx");
                    wait_for_pending_service(handle.service(), expected_pending).await;
                }
                for tx in txs {
                    let id = tx.proposal_short_id();
                    let (_, c) = handle
                        .service()
                        .pool
                        .tx_pool
                        .read()
                        .await
                        .get_tx_with_cycles(&id)
                        .expect("measured tx should be in pool");
                    cycles.insert(tx.hash(), c);
                }
            }
            MeasureMode::PerTxTest => {
                for tx in txs {
                    let c = handle
                        .service()
                        .test_accept_tx(tx.clone())
                        .await
                        .expect("measure cycles via test_accept_tx")
                        .cycles;
                    cycles.insert(tx.hash(), c);
                }
            }
        }
        cycles
    });
    drop(handle);
    cycles
}

// ---------------------------------------------------------------------------
// Transaction builders
// ---------------------------------------------------------------------------

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

fn bundled_cell(key: &str) -> (CellOutput, Bytes) {
    let raw_data = BUNDLED_CELL.get(key).expect("load bundled cell");
    let data: Bytes = raw_data.to_vec().into();
    let cell = CellOutput::new_builder()
        .capacity(Capacity::bytes(data.len()).unwrap())
        .build();
    (cell, data)
}

fn create_secp_system_tx() -> TransactionView {
    let (code_cell, code_data) = bundled_cell("specs/cells/secp256k1_blake160_sighash_all");
    let (data_cell, data_data) = bundled_cell("specs/cells/secp256k1_data");
    let script = secp_script();
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

fn secp_test_consensus(
    issue_outputs: usize,
) -> (Consensus, TransactionView, Vec<OutPoint>, Vec<CellDep>) {
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

    let dao = genesis_dao_data(vec![&system_tx, &issue_tx]).unwrap();
    let genesis = BlockBuilder::default()
        .timestamp(1_557_310_743u64)
        .compact_target(difficulty_to_compact(U256::from(1000u64)))
        .dao(dao)
        .transaction(system_tx)
        .transaction(issue_tx.clone())
        .build();

    let consensus = ConsensusBuilder::default()
        .genesis_block(genesis)
        .cellbase_maturity(EpochNumberWithFraction::new(0, 0, 1))
        .build();

    (consensus, issue_tx, issue_out_points, cell_deps)
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

fn build_single_dependent_chain(shared: &SharedBench, count: usize) -> Vec<TransactionView> {
    let root = shared
        .issue_out_points(1)
        .pop()
        .expect("at least one issue output");
    let mut txs: Vec<TransactionView> = Vec::with_capacity(count);
    for i in 0..count {
        let input = if i == 0 {
            root.clone()
        } else {
            OutPoint::new(txs[i - 1].hash(), 0)
        };
        let tx = if let Some(deps) = &shared.secp_cell_deps {
            build_secp_tx(&input, deps, SECP_ISSUE_CAPACITY)
        } else {
            build_tx(&input, ALWAYS_SUCCESS_ISSUE_CAPACITY as usize)
        };
        txs.push(tx);
    }
    txs
}

// ---------------------------------------------------------------------------
// BenchData — pre-built transactions and cycle counts for one tx type
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum TxType {
    AlwaysSuccess,
    Secp256k1,
    DependentAlwaysSuccess,
    DependentSecp,
}

impl TxType {
    fn as_str(self) -> &'static str {
        match self {
            TxType::AlwaysSuccess => "always_success",
            TxType::Secp256k1 => "secp256k1",
            TxType::DependentAlwaysSuccess => "dependent_always_success",
            TxType::DependentSecp => "dependent_secp",
        }
    }

    fn is_dependent(self) -> bool {
        matches!(self, TxType::DependentAlwaysSuccess | TxType::DependentSecp)
    }
}

struct BenchData {
    shared: SharedBench,
    txs: Vec<TransactionView>,
    cycles: HashMap<ckb_types::packed::Byte32, u64>,
    tx_type: TxType,
    warm_pool_size: usize,
}

impl BenchData {
    fn new(tx_type: TxType, max_size: usize, warm_pool_size: usize) -> Self {
        let issue_outputs = max_size + warm_pool_size;
        let (shared, txs, cycles) = match tx_type {
            TxType::AlwaysSuccess => {
                let shared = SharedBench::new_always_success(issue_outputs);
                let txs: Vec<_> = shared
                    .issue_out_points(issue_outputs)
                    .iter()
                    .map(|out_point| build_tx(out_point, 4_000))
                    .collect();
                // always_success verification cost is deterministic, so measure
                // one sample and apply the same cycle count to all txs.
                let cycles = measure_cycles(&shared, &txs, MeasureMode::Uniform);
                (shared, txs, cycles)
            }
            TxType::Secp256k1 => {
                let (shared, cell_deps) = SharedBench::new_secp(issue_outputs);
                let txs: Vec<_> = shared
                    .issue_out_points(issue_outputs)
                    .iter()
                    .map(|out_point| {
                        build_secp_tx(out_point, &cell_deps, SECP_ISSUE_CAPACITY - SECP_FEE)
                    })
                    .collect();
                let cycles = measure_cycles(&shared, &txs, MeasureMode::PerTxTest);
                (shared, txs, cycles)
            }
            TxType::DependentAlwaysSuccess => {
                let shared = SharedBench::new_always_success(issue_outputs);
                let txs = build_single_dependent_chain(&shared, issue_outputs);
                let cycles = measure_cycles(&shared, &txs, MeasureMode::PerTxProcess);
                (shared, txs, cycles)
            }
            TxType::DependentSecp => {
                let (shared, _) = SharedBench::new_secp(issue_outputs);
                let txs = build_single_dependent_chain(&shared, issue_outputs);
                let cycles = measure_cycles(&shared, &txs, MeasureMode::PerTxProcess);
                (shared, txs, cycles)
            }
        };
        Self {
            shared,
            txs,
            cycles,
            tx_type,
            warm_pool_size,
        }
    }

    fn warm(&self) -> (Arc<Vec<TransactionView>>, Arc<Vec<u64>>) {
        let txs: Vec<_> = self.txs[..self.warm_pool_size].to_vec();
        let cycles = txs
            .iter()
            .map(|tx| *self.cycles.get(&tx.hash()).expect("missing cycle"))
            .collect();
        (Arc::new(txs), Arc::new(cycles))
    }

    fn target(&self, size: usize) -> (Arc<Vec<TransactionView>>, Arc<Vec<u64>>) {
        let end = self.warm_pool_size + size;
        let txs: Vec<_> = self.txs[self.warm_pool_size..end].to_vec();
        let cycles = txs
            .iter()
            .map(|tx| *self.cycles.get(&tx.hash()).expect("missing cycle"))
            .collect();
        (Arc::new(txs), Arc::new(cycles))
    }
}

// ---------------------------------------------------------------------------
// Submit and wait
// ---------------------------------------------------------------------------

fn submit_and_wait(
    runtime: &tokio::runtime::Runtime,
    controller: &crate::TxPoolController,
    completion: &BenchCompletion,
    txs: Arc<Vec<TransactionView>>,
    cycles: Arc<Vec<u64>>,
    target_pending: usize,
    submitters: usize,
) {
    runtime.block_on(async {
        // Split into per-submitter ranges.  Each spawned task gets a cheap
        // Arc clone and indexes directly into the shared Vec, avoiding a
        // full Vec clone per submitter.
        let chunk_size = if submitters == 0 || txs.is_empty() {
            txs.len()
        } else {
            txs.len().div_ceil(submitters)
        };
        let ranges: Vec<(usize, usize)> = (0..txs.len())
            .step_by(chunk_size.max(1))
            .map(|start| (start, (start + chunk_size).min(txs.len())))
            .collect();

        let mut handles = Vec::with_capacity(ranges.len());
        for (start, end) in ranges {
            let controller = controller.clone();
            let txs = Arc::clone(&txs);
            let cycles = Arc::clone(&cycles);
            handles.push(tokio::spawn(async move {
                for i in start..end {
                    controller
                        .submit_remote_tx(txs[i].clone(), cycles[i], 1.into())
                        .await
                        .expect("submit remote tx");
                }
            }));
        }
        for h in handles {
            h.await.expect("submitter");
        }
        completion.wait_for(target_pending).await;
    })
}

// ---------------------------------------------------------------------------
// Benchmark matrix
// ---------------------------------------------------------------------------

// Full matrix: all combinations — runs in ~30+ minutes.
const SIZES: &[usize] = &[50, 100];
const PEER_COUNTS: &[usize] = &[1, 2, 4, 8];
const WORKER_COUNTS: &[usize] = &[4, 8, 12];
const WARM_POOL_SIZE: usize = 30;
const DEPENDENT_SIZES: &[usize] = &[10, 20];
const DEPENDENT_WARM_POOL_SIZE: usize = 10;

// Medium matrix: a balanced tier — runs in roughly 10–15 minutes.
// This is the **default** matrix (no env var needed).
const MEDIUM_SIZES: &[usize] = &[100];
const MEDIUM_PEER_COUNTS: &[usize] = &[1, 4];
const MEDIUM_WORKER_COUNTS: &[usize] = &[4, 8];
const MEDIUM_WARM_POOL_SIZE: usize = 30;
const MEDIUM_DEPENDENT_SIZES: &[usize] = &[10];
const MEDIUM_DEPENDENT_WARM_POOL_SIZE: usize = 10;

// Quick matrix: runs in about 5 minutes — activate with QUICK_BENCH=1.
const QUICK_SIZES: &[usize] = &[100];
const QUICK_PEER_COUNTS: &[usize] = &[1];
const QUICK_WORKER_COUNTS: &[usize] = &[8];
const QUICK_WARM_POOL_SIZE: usize = 30;
const QUICK_DEPENDENT_SIZES: &[usize] = &[20];
const QUICK_DEPENDENT_WARM_POOL_SIZE: usize = 10;

struct BenchMatrix {
    sizes: &'static [usize],
    peer_counts: &'static [usize],
    worker_counts: &'static [usize],
    warm_pool_size: usize,
    dependent_sizes: &'static [usize],
    dependent_warm_pool_size: usize,
}

fn bench_matrix() -> BenchMatrix {
    if std::env::var("QUICK_BENCH").is_ok() {
        BenchMatrix {
            sizes: QUICK_SIZES,
            peer_counts: QUICK_PEER_COUNTS,
            worker_counts: QUICK_WORKER_COUNTS,
            warm_pool_size: QUICK_WARM_POOL_SIZE,
            dependent_sizes: QUICK_DEPENDENT_SIZES,
            dependent_warm_pool_size: QUICK_DEPENDENT_WARM_POOL_SIZE,
        }
    } else if std::env::var("FULL_BENCH").is_ok() {
        BenchMatrix {
            sizes: SIZES,
            peer_counts: PEER_COUNTS,
            worker_counts: WORKER_COUNTS,
            warm_pool_size: WARM_POOL_SIZE,
            dependent_sizes: DEPENDENT_SIZES,
            dependent_warm_pool_size: DEPENDENT_WARM_POOL_SIZE,
        }
    } else {
        // Default: medium matrix for a good speed/signal trade-off.
        BenchMatrix {
            sizes: MEDIUM_SIZES,
            peer_counts: MEDIUM_PEER_COUNTS,
            worker_counts: MEDIUM_WORKER_COUNTS,
            warm_pool_size: MEDIUM_WARM_POOL_SIZE,
            dependent_sizes: MEDIUM_DEPENDENT_SIZES,
            dependent_warm_pool_size: MEDIUM_DEPENDENT_WARM_POOL_SIZE,
        }
    }
}

// ---------------------------------------------------------------------------
// Benchmark registration
// ---------------------------------------------------------------------------

fn register_cold_bench(
    group: &mut BenchmarkGroup<'_, criterion::measurement::WallTime>,
    mode: &str,
    data: &BenchData,
    peers: usize,
    workers: usize,
    size: usize,
    reverse_dependent: bool,
) {
    let (mut txs, mut cycles) = data.target(size);
    // Benchmark both dependency paths explicitly. Parent-first traffic uses
    // FlightTracker + OrderedResolveQueue; child-first traffic uses orphan
    // parking and cascade recovery. Historically only the latter was measured,
    // which could hide a regression in the normal dependent fast path.
    if data.tx_type.is_dependent() && reverse_dependent {
        let txs_mut = Arc::make_mut(&mut txs);
        txs_mut.reverse();
        let cycles_mut = Arc::make_mut(&mut cycles);
        cycles_mut.reverse();
    }
    let tx_type = if data.tx_type.is_dependent() {
        format!(
            "{}_{}",
            data.tx_type.as_str(),
            if reverse_dependent {
                "child_first"
            } else {
                "parent_first"
            }
        )
    } else {
        data.tx_type.as_str().to_string()
    };
    group.throughput(Throughput::Elements(size as u64));

    if data.tx_type.is_dependent() {
        // Cold dependent txs depend on the warm prefix of the chain.  Pre-submit
        // that prefix (not measured) so the reversed target txs have parents in
        // the pool.
        let (warm_txs, warm_cycles) = data.warm();
        let expected_pending = data.warm_pool_size + size;
        group.bench_function(
            format!("{mode}_{peers}peer_{workers}worker_{tx_type}_{size}"),
            |b| {
                b.iter_batched_ref(
                    || {
                        let handle = data.shared.start_controller(workers);
                        submit_and_wait(
                            &data.shared.runtime,
                            &handle.controller,
                            &handle.completion,
                            Arc::clone(&warm_txs),
                            Arc::clone(&warm_cycles),
                            data.warm_pool_size,
                            1,
                        );
                        handle
                    },
                    |handle| {
                        submit_and_wait(
                            &data.shared.runtime,
                            &handle.controller,
                            &handle.completion,
                            Arc::clone(&txs),
                            Arc::clone(&cycles),
                            expected_pending,
                            peers,
                        )
                    },
                    BatchSize::PerIteration,
                )
            },
        );
    } else {
        group.bench_function(
            format!("{mode}_{peers}peer_{workers}worker_{tx_type}_{size}"),
            |b| {
                b.iter_batched_ref(
                    || data.shared.start_controller(workers),
                    |handle| {
                        submit_and_wait(
                            &data.shared.runtime,
                            &handle.controller,
                            &handle.completion,
                            Arc::clone(&txs),
                            Arc::clone(&cycles),
                            size,
                            peers,
                        )
                    },
                    BatchSize::PerIteration,
                )
            },
        );
    }
}

fn register_warm_bench(
    group: &mut BenchmarkGroup<'_, criterion::measurement::WallTime>,
    mode: &str,
    data: &BenchData,
    peers: usize,
    workers: usize,
    size: usize,
    reverse_dependent: bool,
) {
    let (warm_txs, warm_cycles) = data.warm();
    let (mut target_txs, mut target_cycles) = data.target(size);
    if data.tx_type.is_dependent() && reverse_dependent {
        Arc::make_mut(&mut target_txs).reverse();
        Arc::make_mut(&mut target_cycles).reverse();
    }
    let expected_pending = data.warm_pool_size + size;
    let tx_type = if data.tx_type.is_dependent() {
        format!(
            "{}_{}",
            data.tx_type.as_str(),
            if reverse_dependent {
                "child_first"
            } else {
                "parent_first"
            }
        )
    } else {
        data.tx_type.as_str().to_string()
    };
    group.throughput(Throughput::Elements(size as u64));
    // The setup closure (iter_batched_ref) creates a fresh controller with an empty
    // verify cache, then submits warm_txs which populate the cache with those
    // entries.  The measured closure then submits target_txs (different hashes),
    // so they miss the cache and undergo full verification — matching the real
    // behaviour of a node that already holds verified txs in its pool.
    group.bench_function(
        format!("{mode}_{peers}peer_{workers}worker_warm_{tx_type}_{size}"),
        |b| {
            b.iter_batched_ref(
                || {
                    let handle = data.shared.start_controller(workers);
                    submit_and_wait(
                        &data.shared.runtime,
                        &handle.controller,
                        &handle.completion,
                        Arc::clone(&warm_txs),
                        Arc::clone(&warm_cycles),
                        data.warm_pool_size,
                        peers,
                    );
                    handle
                },
                |handle| {
                    submit_and_wait(
                        &data.shared.runtime,
                        &handle.controller,
                        &handle.completion,
                        Arc::clone(&target_txs),
                        Arc::clone(&target_cycles),
                        expected_pending,
                        peers,
                    )
                },
                BatchSize::PerIteration,
            )
        },
    );
}

// ---------------------------------------------------------------------------
// Top-level benchmark entry point
// ---------------------------------------------------------------------------

fn bench(c: &mut Criterion) {
    let mode = "pipeline";

    let matrix = bench_matrix();

    let non_dep_max = *matrix.sizes.iter().max().expect("sizes is non-empty");
    let mut data_sets = vec![
        (
            BenchData::new(TxType::AlwaysSuccess, non_dep_max, matrix.warm_pool_size),
            matrix.sizes,
        ),
        (
            BenchData::new(TxType::Secp256k1, non_dep_max, matrix.warm_pool_size),
            matrix.sizes,
        ),
    ];

    // Dependent chain cycle measurement (PerTxProcess) relies on the pipeline's
    // classify → ordered-resolve → verify path.
    {
        let dep_max = *matrix
            .dependent_sizes
            .iter()
            .max()
            .expect("dependent_sizes is non-empty");
        data_sets.push((
            BenchData::new(
                TxType::DependentAlwaysSuccess,
                dep_max,
                matrix.dependent_warm_pool_size,
            ),
            matrix.dependent_sizes,
        ));
        data_sets.push((
            BenchData::new(
                TxType::DependentSecp,
                dep_max,
                matrix.dependent_warm_pool_size,
            ),
            matrix.dependent_sizes,
        ));
    }

    eprintln!(
        "Running tx-pool benchmark in '{}' mode, sizes {:?}, dependent_sizes {:?}, peers {:?}, workers {:?}",
        mode, matrix.sizes, matrix.dependent_sizes, matrix.peer_counts, matrix.worker_counts
    );

    let quick = std::env::var("QUICK_BENCH").is_ok();
    let full = std::env::var("FULL_BENCH").is_ok();

    let mut group = c.benchmark_group("tx_pool_pipeline");
    if quick {
        // Keep the matrix narrow, but collect enough work per scenario to
        // amortize scheduler jitter. This remains much faster than medium
        // because it has one peer/worker combination instead of four.
        group.sample_size(20);
        group.sampling_mode(SamplingMode::Flat);
        group.warm_up_time(Duration::from_secs(2));
        group.measurement_time(Duration::from_secs(8));
    } else if full {
        group.sample_size(50);
        group.warm_up_time(Duration::from_secs(5));
        group.measurement_time(Duration::from_secs(15));
    } else {
        // Default (medium) matrix: larger sample size and explicit timing for
        // tighter confidence intervals on regression detection.
        group.sample_size(30);
        group.warm_up_time(Duration::from_secs(3));
        group.measurement_time(Duration::from_secs(10));
    }

    // Dependent txs are bottlenecked by chain-serialized orphan recovery, so
    // varying peer/worker counts produces no meaningful signal.  Only benchmark
    // dependent types once with 1 peer and the first worker count.
    let dep_peers = 1;
    let dep_workers = *matrix
        .worker_counts
        .first()
        .expect("worker_counts is non-empty");

    for workers in matrix.worker_counts {
        for peers in matrix.peer_counts {
            for (data, sizes) in &data_sets {
                if data.tx_type.is_dependent() && (*peers != dep_peers || *workers != dep_workers) {
                    continue;
                }
                let dependency_orders: &[bool] = if data.tx_type.is_dependent() {
                    &[false, true]
                } else {
                    &[false]
                };
                for size in *sizes {
                    for reverse_dependent in dependency_orders {
                        register_cold_bench(
                            &mut group,
                            mode,
                            data,
                            *peers,
                            *workers,
                            *size,
                            *reverse_dependent,
                        );
                    }
                }
            }
        }
    }

    for workers in matrix.worker_counts {
        for peers in matrix.peer_counts {
            for (data, sizes) in &data_sets {
                if data.tx_type.is_dependent() && (*peers != dep_peers || *workers != dep_workers) {
                    continue;
                }
                let dependency_orders: &[bool] = if data.tx_type.is_dependent() {
                    &[false, true]
                } else {
                    &[false]
                };
                for size in *sizes {
                    for reverse_dependent in dependency_orders {
                        register_warm_bench(
                            &mut group,
                            mode,
                            data,
                            *peers,
                            *workers,
                            *size,
                            *reverse_dependent,
                        );
                    }
                }
            }
        }
    }

    group.finish();
}

#[allow(missing_docs)]
mod benches {
    use super::*;
    criterion_group! {
        name = pipeline_bench;
        config = Criterion::default();
        targets = bench
    }
}

pub use benches::pipeline_bench;

#[cfg(test)]
mod debug_tests {
    use super::*;

    #[test]
    fn controller_dependent_secp_chain_reverse() {
        let (mut shared, cell_deps) = SharedBench::new_secp(2);
        shared.secp_cell_deps = Some(cell_deps);
        let mut txs = build_single_dependent_chain(&shared, 2);
        let cycles = measure_cycles(&shared, &txs, MeasureMode::PerTxProcess);
        txs.reverse();
        eprintln!("dependent secp chain cycles {:?}", cycles);
        let handle = shared.start_controller(4);
        shared.runtime.block_on(async {
            for tx in txs.iter() {
                let c = cycles.get(&tx.hash()).copied().expect("missing cycle");
                handle
                    .controller
                    .submit_remote_tx(tx.clone(), c, 1.into())
                    .await
                    .expect("submit");
            }
            for i in 0..200 {
                let info = handle.controller.get_tx_pool_info().unwrap();
                eprintln!(
                    "iter {i} pending={} orphan={}",
                    info.pending_size, info.orphan_size
                );
                if info.pending_size >= 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert!(handle.controller.get_tx_pool_info().unwrap().pending_size >= 2);
        });
    }
}
