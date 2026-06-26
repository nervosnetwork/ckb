//! Criterion benchmark for the tx-pool resolve -> verify -> submit pipeline.
//!
//! This module is only available with the `internal` feature. The actual binary
//! target lives in `tx-pool/benches/pipeline.rs`.
//!
//! # Comparing sync vs pipeline
//!
//! Run each mode separately and then compare with Criterion's built-in
//! baseline facility:
//!
//! ```sh
//! cargo bench --no-default-features --features internal --bench pipeline -- --save-baseline sync
//! cargo bench --features "internal pipeline" --bench pipeline -- --save-baseline pipeline
//! critcmp sync.json pipeline.json   # install critcmp with `cargo install critcmp`
//! ```

use crate::resolve_mgr::{OrderedResolver, ResolveExit};
use crate::service::TxPoolService;
use ckb_app_config::{NetworkConfig, TxPoolConfig};
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_crypto::secp::Privkey;
use ckb_dao_utils::genesis_dao_data;
use ckb_fee_estimator::FeeEstimator;
use ckb_network::{Flags, NetworkService, NetworkState, network::TransportType};
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
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::{RwLock, watch};

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

fn network(consensus: &Consensus) -> (ckb_network::NetworkController, TempDir) {
    let handle = ckb_async_runtime::new_background_runtime();
    let tmp_dir = TempDir::new().expect("create temp dir");
    let config = NetworkConfig {
        max_peers: 19,
        max_outbound_peers: 5,
        path: tmp_dir.path().to_path_buf(),
        ping_interval_secs: 15,
        ping_timeout_secs: 20,
        connect_outbound_interval_secs: 1,
        discovery_local_address: true,
        bootnode_mode: true,
        reuse_port_on_linux: true,
        ..Default::default()
    };
    let network_state =
        Arc::new(NetworkState::from_config(config).expect("init test network state"));
    let controller = NetworkService::new(
        network_state,
        vec![],
        vec![],
        (consensus.identify_name(), "test".to_string(), Flags::all()),
        TransportType::Tcp,
    )
    .start(&handle)
    .expect("start test network service");
    (controller, tmp_dir)
}

// ---------------------------------------------------------------------------
// SharedBench — resources shared across all benchmark iterations
// ---------------------------------------------------------------------------

struct SharedBench {
    _store: MockStore,
    _network_tmp_dir: TempDir,
    issue_tx: TransactionView,
    snapshot: Arc<Snapshot>,
    network: ckb_network::NetworkController,
    runtime: tokio::runtime::Runtime,
    ckb_handle: ckb_async_runtime::Handle,
    secp_cell_deps: Option<Vec<CellDep>>,
}

impl SharedBench {
    fn from_consensus(
        consensus: Consensus,
        issue_tx: TransactionView,
        secp_cell_deps: Option<Vec<CellDep>>,
    ) -> Self {
        let consensus = Arc::new(consensus);
        let (store, snapshot) = snapshot_with_genesis(Arc::clone(&consensus));
        let (network, network_tmp_dir) = network(&consensus);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(8)
            .enable_all()
            .build()
            .expect("build tokio runtime");
        let ckb_handle = ckb_async_runtime::Handle::new(runtime.handle().clone(), None);
        Self {
            _store: store,
            _network_tmp_dir: network_tmp_dir,
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
    /// dropping it closes the relay channel, allowing the background drain task
    /// to exit cleanly.
    fn start_controller(&self, max_workers: usize) -> ServiceHandle {
        let config = tx_pool_config(max_workers);
        let (tx_relay_sender, tx_relay_receiver) = ckb_channel::bounded(1024);

        // Drain relay results so the channel behaves like a real relayer consumer.
        self.runtime
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
        // Replace the global exit token with a local one so stopping this service
        // does not affect other benchmark iterations.
        let local_signal = CancellationToken::new();
        builder.signal_receiver = local_signal.clone();
        builder.start(self.network.clone());
        ServiceHandle {
            controller,
            signal: local_signal,
            tx_relay_sender: Some(tx_relay_sender),
        }
    }
}

/// RAII handle for a benchmark service instance.
///
/// On drop it cancels the [`CancellationToken`] (shutting down all spawned
/// workers) **and** drops the `tx_relay_sender` clone, which causes the
/// background relay-drain task to exit cleanly.
struct ServiceHandle {
    controller: crate::TxPoolController,
    signal: CancellationToken,
    /// Stored so we can explicitly drop it on shutdown, closing the relay
    /// channel and letting the background drain task exit.
    tx_relay_sender: Option<ckb_channel::Sender<crate::service::TxVerificationResult>>,
}

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        self.signal.cancel();
        // Drop the sender explicitly so the relay drain task (spawn_blocking)
        // sees a closed channel and exits instead of leaking.
        self.tx_relay_sender.take();
    }
}

// ---------------------------------------------------------------------------
// Async helpers
// ---------------------------------------------------------------------------

/// Poll the tx-pool until at least `count` transactions reach the pending state.
///
/// Uses a 5 ms polling interval for tight feedback on small batches while still
/// being gentle on the runtime scheduler.
async fn wait_for_pending(controller: &crate::TxPoolController, count: usize) {
    let start = Instant::now();
    let mut last_log = start;
    let result = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let info = controller.get_tx_pool_info().ok();
            let pending = info.as_ref().map(|i| i.pending_size).unwrap_or(0);
            if pending >= count {
                break;
            }
            let now = Instant::now();
            if now.duration_since(last_log).as_secs() >= 5 {
                eprintln!(
                    "[wait_for_pending] pending={} orphan={} total={} after {:?}",
                    pending,
                    info.as_ref().map(|i| i.orphan_size).unwrap_or(0),
                    info.as_ref().map(|i| i.total_tx_size).unwrap_or(0),
                    now.duration_since(start)
                );
                last_log = now;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    let pending = controller
        .get_tx_pool_info()
        .map(|info| info.pending_size)
        .unwrap_or(usize::MAX);
    assert!(
        result.is_ok(),
        "pipeline did not process all txs in time: {}/{} pending",
        pending,
        count
    );
}

// ---------------------------------------------------------------------------
// start_service — used by measure_cycles (needs direct TxPoolService access)
// ---------------------------------------------------------------------------

/// Build a [`TxPoolService`] together with its pipeline workers for direct
/// method calls (e.g. `process_tx`, `_test_accept_tx`).
///
/// This uses [`TxPoolServiceBuilder::build_bench_service`] for the service
/// construction, ensuring the same assembly path as production code.
fn start_service(
    shared: &SharedBench,
    max_workers: usize,
) -> (TxPoolService, ckb_stop_handler::CancellationToken) {
    let config = tx_pool_config(max_workers);
    let (tx_relay_sender, tx_relay_receiver) = ckb_channel::bounded(1024);
    shared
        .runtime
        .spawn_blocking(move || while tx_relay_receiver.recv().is_ok() {});

    let (builder, _controller) = crate::TxPoolServiceBuilder::new(
        config,
        Arc::clone(&shared.snapshot),
        None,
        Arc::new(RwLock::new(init_cache())),
        &shared.ckb_handle,
        tx_relay_sender,
        FeeEstimator::new_dummy(),
    );
    let parts = builder.build_bench_service(shared.network.clone());

    // Spawn the deferred task worker (recovery tx re-enqueue + cache updates).
    {
        let svc = parts.service.clone();
        let mut deferred_rx = parts.deferred_receiver;
        tokio::spawn(async move {
            while let Some(task) = deferred_rx.recv().await {
                match task {
                    crate::service::DeferredTask::RecoverTxs(txs) => {
                        let mut queue = svc.ordered_resolve_queue.write().await;
                        for tx in txs {
                            let _ = queue.add_tx(crate::resolved_tx::ResolveJob {
                                tx,
                                remote: None,
                                is_proposal_tx: false,
                                attempts: 0,
                            });
                        }
                    }
                    crate::service::DeferredTask::CacheUpdate {
                        wtx_hash,
                        verified,
                    } => {
                        let mut guard = svc.txs_verify_cache.write().await;
                        guard.put(wtx_hash, verified);
                    }
                }
            }
        });
    }

    // Spawn pipeline workers using the components from the builder.
    #[cfg(feature = "pipeline")]
    let signal = parts.signal;
    #[cfg(not(feature = "pipeline"))]
    let signal = ckb_stop_handler::CancellationToken::new();

    #[cfg(feature = "pipeline")]
    {
        let pre_check_workers =
            max_workers.min(std::thread::available_parallelism().map_or(4, |n| n.get()));
        for _ in 0..pre_check_workers {
            let svc = parts.service.clone();
            let queue = Arc::clone(&parts.pre_check_queue);
            tokio::spawn(async move {
                while let Some(job) = queue.pop().await {
                    let _ = svc
                        .classify_and_enqueue_tx(job.tx, job.is_proposal_tx, job.remote)
                        .await;
                }
            });
        }
    }

    // Create a fresh chunk channel shared by VerifyMgr and OrderedResolver.
    // The builder's original chunk_tx was inside the TxPoolController which
    // was consumed by build_bench_service, so we need our own.
    let (chunk_tx, chunk_rx) = watch::channel(ChunkCommand::Resume);

    let mut verify_mgr =
        crate::verify_mgr::VerifyMgr::new(parts.service.clone(), chunk_rx, signal.child_token());
    tokio::spawn(async move { verify_mgr.run().await });

    let ordered_resolver = OrderedResolver::new(
        parts.service.clone(),
        Arc::clone(&parts.service.ordered_resolve_queue),
        Arc::clone(&parts.service.verify_queue),
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

    (parts.service, signal)
}

// ---------------------------------------------------------------------------
// Cycle measurement
// ---------------------------------------------------------------------------

/// How cycle counts should be measured for a set of transactions.
enum MeasureMode {
    /// Measure one sample tx via `_test_accept_tx` and apply the same cycle
    /// count to every transaction in the set (appropriate for always_success
    /// scripts whose verification cost is deterministic).
    Uniform,
    /// Measure each tx individually via `process_tx`.  Required for dependent
    /// chains whose inputs resolve through the pool's locked path.
    PerTxProcess,
    /// Measure each tx individually via `_test_accept_tx`.  Required for
    /// independent secp256k1 txs whose ECDSA verification cycles vary
    /// non-deterministically with signature values.
    PerTxTest,
}

/// Measure verification cycles for a set of transactions.
///
/// For dependent chains, `process_tx` is used because it resolves inputs
/// through both the chain snapshot **and** the tx-pool (via
/// `pre_check_with_pool_lock`), allowing earlier transactions in the chain to
/// serve as parents for later ones.  Without this, dependent children would
/// fail resolution since their inputs are not yet on-chain.
///
/// secp256k1 ECDSA verification has non-deterministic cycle counts per tx
/// (different signature values take different CKB-VM execution paths), so each
/// tx must be measured individually — we cannot just measure one and multiply.
fn measure_cycles(
    shared: &SharedBench,
    txs: &[TransactionView],
    mode: MeasureMode,
) -> HashMap<ckb_types::packed::Byte32, u64> {
    let (service, signal) = shared.runtime.block_on(async { start_service(shared, 8) });
    let cycles = shared.runtime.block_on(async {
        let mut cycles = HashMap::with_capacity(txs.len());
        match mode {
            MeasureMode::Uniform => {
                let sample = &txs[0];
                let c = service
                    ._test_accept_tx(sample.clone())
                    .await
                    .expect("measure uniform cycle")
                    .cycles;
                for tx in txs {
                    cycles.insert(tx.hash(), c);
                }
            }
            MeasureMode::PerTxProcess => {
                for tx in txs {
                    let c = service
                        .process_tx(tx.clone(), None)
                        .await
                        .expect("measure cycles via process_tx")
                        .cycles;
                    cycles.insert(tx.hash(), c);
                }
            }
            MeasureMode::PerTxTest => {
                for tx in txs {
                    let c = service
                        ._test_accept_tx(tx.clone())
                        .await
                        .expect("measure cycles via _test_accept_tx")
                        .cycles;
                    cycles.insert(tx.hash(), c);
                }
            }
        }
        cycles
    });
    signal.cancel();
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

fn build_dependent_chain(shared: &SharedBench, count: usize) -> Vec<TransactionView> {
    let roots = shared.issue_out_points(count);
    let mut txs: Vec<TransactionView> = Vec::with_capacity(count);
    for (i, root) in roots.iter().enumerate() {
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
// Pipeline metrics (P3-9)
// ---------------------------------------------------------------------------

/// Snapshot of internal pipeline queue depths at a point in time.
#[derive(Default, Clone, Debug)]
#[allow(dead_code)]
struct PipelineMetrics {
    pending: usize,
    orphan_size: usize,
}

#[allow(dead_code)]
impl PipelineMetrics {
    /// Collect metrics directly from the service's internal state (used by
    /// `measure_cycles` where no controller exists).
    fn snapshot_from_service(service: &TxPoolService) -> Self {
        let orphan_size = service
            .orphan
            .try_read()
            .map(|o| o.len())
            .unwrap_or(0);
        Self {
            pending: 0, // not easily accessible without a controller
            orphan_size,
        }
    }

    fn snapshot(controller: &crate::TxPoolController) -> Self {
        let info = controller.get_tx_pool_info().ok();
        Self {
            pending: info.as_ref().map(|i| i.pending_size).unwrap_or(0),
            orphan_size: info.as_ref().map(|i| i.orphan_size).unwrap_or(0),
        }
    }
}

impl std::fmt::Display for PipelineMetrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "pending={} orphan={}",
            self.pending, self.orphan_size,
        )
    }
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
                let txs = build_dependent_chain(&shared, issue_outputs);
                let cycles = measure_cycles(&shared, &txs, MeasureMode::PerTxProcess);
                (shared, txs, cycles)
            }
            TxType::DependentSecp => {
                let (shared, _) = SharedBench::new_secp(issue_outputs);
                let txs = build_dependent_chain(&shared, issue_outputs);
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

        let controller_for_wait = controller.clone();
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
        wait_for_pending(&controller_for_wait, target_pending).await;
        for h in handles {
            h.await.expect("submitter");
        }
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
const QUICK_SIZES: &[usize] = &[50];
const QUICK_PEER_COUNTS: &[usize] = &[1];
const QUICK_WORKER_COUNTS: &[usize] = &[8];
const QUICK_WARM_POOL_SIZE: usize = 50;
const QUICK_DEPENDENT_SIZES: &[usize] = &[10];
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
) {
    let (mut txs, mut cycles) = data.target(size);
    // Submit dependent chains in reverse order so children land in the orphan
    // pool and are recovered after their parents are accepted.  Submitting in
    // natural order would route them to the ordered resolve queue, which is not
    // re-driven once the parent leaves the pipeline.
    if data.tx_type.is_dependent() {
        let txs_mut = Arc::make_mut(&mut txs);
        txs_mut.reverse();
        let cycles_mut = Arc::make_mut(&mut cycles);
        cycles_mut.reverse();
    }
    let tx_type = data.tx_type.as_str();
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
                b.iter_batched(
                    || {
                        let handle = data.shared.start_controller(workers);
                        submit_and_wait(
                            &data.shared.runtime,
                            &handle.controller,
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
                b.iter_batched(
                    || data.shared.start_controller(workers),
                    |handle| {
                        submit_and_wait(
                            &data.shared.runtime,
                            &handle.controller,
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
) {
    let (warm_txs, warm_cycles) = data.warm();
    let (target_txs, target_cycles) = data.target(size);
    let expected_pending = data.warm_pool_size + size;
    let tx_type = data.tx_type.as_str();
    group.throughput(Throughput::Elements(size as u64));
    // The setup closure (iter_batched) creates a fresh controller with an empty
    // verify cache, then submits warm_txs which populate the cache with those
    // entries.  The measured closure then submits target_txs (different hashes),
    // so they miss the cache and undergo full verification — matching the real
    // behaviour of a node that already holds verified txs in its pool.
    group.bench_function(
        format!("{mode}_{peers}peer_{workers}worker_warm_{tx_type}_{size}"),
        |b| {
            b.iter_batched(
                || {
                    let handle = data.shared.start_controller(workers);
                    submit_and_wait(
                        &data.shared.runtime,
                        &handle.controller,
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
    let mode = if cfg!(feature = "pipeline") {
        "pipeline"
    } else {
        "sync"
    };

    let matrix = bench_matrix();

    let non_dep_max = *matrix.sizes.iter().max().expect("sizes is non-empty");
    let dep_max = *matrix
        .dependent_sizes
        .iter()
        .max()
        .expect("dependent_sizes is non-empty");
    let data_sets = vec![
        (
            BenchData::new(TxType::AlwaysSuccess, non_dep_max, matrix.warm_pool_size),
            matrix.sizes,
        ),
        (
            BenchData::new(TxType::Secp256k1, non_dep_max, matrix.warm_pool_size),
            matrix.sizes,
        ),
        (
            BenchData::new(
                TxType::DependentAlwaysSuccess,
                dep_max,
                matrix.dependent_warm_pool_size,
            ),
            matrix.dependent_sizes,
        ),
        (
            BenchData::new(
                TxType::DependentSecp,
                dep_max,
                matrix.dependent_warm_pool_size,
            ),
            matrix.dependent_sizes,
        ),
    ];

    eprintln!(
        "Running tx-pool benchmark in '{}' mode, sizes {:?}, dependent_sizes {:?}, peers {:?}, workers {:?}",
        mode, matrix.sizes, matrix.dependent_sizes, matrix.peer_counts, matrix.worker_counts
    );

    let mut group = c.benchmark_group("tx_pool_pipeline");
    group.sample_size(10);
    group.sampling_mode(SamplingMode::Flat);

    // Dependent txs are bottlenecked by chain-serialized orphan recovery, so
    // varying peer/worker counts produces no meaningful signal.  Only benchmark
    // dependent types once with 1 peer and the first worker count.
    let dep_peers = 1;
    let dep_workers = *matrix.worker_counts.first().expect("worker_counts is non-empty");

    for workers in matrix.worker_counts {
        for peers in matrix.peer_counts {
            for (data, sizes) in &data_sets {
                if data.tx_type.is_dependent() && (*peers != dep_peers || *workers != dep_workers) {
                    continue;
                }
                for size in *sizes {
                    register_cold_bench(&mut group, mode, data, *peers, *workers, *size);
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
                for size in *sizes {
                    register_warm_bench(&mut group, mode, data, *peers, *workers, *size);
                }
            }
        }
    }

    group.finish();
}

criterion_group! {
    name = pipeline_bench;
    config = Criterion::default().sample_size(10);
    targets = bench
}

#[cfg(test)]
mod debug_tests {
    use super::*;

    #[test]
    fn controller_dependent_secp_chain_reverse() {
        let (mut shared, cell_deps) = SharedBench::new_secp(2);
        shared.secp_cell_deps = Some(cell_deps.clone());
        let mut txs = build_dependent_chain(&shared, 2);
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
