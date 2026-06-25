//! Criterion benchmark for the tx-pool resolve -> verify -> submit pipeline.
//!
//! This module is only available with the `internal` feature. The actual binary
//! target lives in `tx-pool/benches/pipeline.rs`.

use crate::component::orphan::OrphanPool;
use crate::component::resolve_queue::ResolveQueue;
use crate::component::verify_queue::VerifyQueue;
use crate::pool::TxPool;
use crate::resolve_mgr::{OrderedResolver, PreResolveMgr, ResolveExit};
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
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{RwLock, mpsc, watch};

const MAX_TX_VERIFY_CYCLES: u64 = 70_000_000;
const ALWAYS_SUCCESS_ISSUE_CAPACITY: u64 = 5_000;

const SECP_PRIVKEY: H256 =
    h256!("0xb2b3324cece882bca684eaf202667bb56ed8e8c2fd4b4dc71f615ebd6d9055a5");
const SECP_PUBKEY_HASH: H160 = h160!("0x779e5930892a0a9bf2fedfe048f685466c7d0396");
const SECP_ISSUE_CAPACITY: u64 = 50_000 * 100_000_000;
const SECP_FEE: u64 = 1_000 * 100_000_000;

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
            tx_relay_sender,
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
        }
    }
}

struct ServiceHandle {
    controller: crate::TxPoolController,
    signal: CancellationToken,
}

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        self.signal.cancel();
    }
}

fn start_service(
    shared: &SharedBench,
    max_workers: usize,
) -> (TxPoolService, ckb_stop_handler::CancellationToken) {
    let config = tx_pool_config(max_workers);
    let tx_pool = TxPool::new(config.clone(), Arc::clone(&shared.snapshot));
    let (tx_relay_sender, tx_relay_receiver) = ckb_channel::bounded(1024);
    // Drain relay results so the temporary service does not block on a full channel.
    shared
        .runtime
        .spawn_blocking(move || while tx_relay_receiver.recv().is_ok() {});
    let (block_assembler_sender, _) = mpsc::channel(1);

    let resolve_queue = Arc::new(RwLock::new(ResolveQueue::new()));
    let ordered_resolve_queue = Arc::new(RwLock::new(
        crate::component::ordered_resolve_queue::OrderedResolveQueue::new(),
    ));
    let verify_queue = Arc::new(RwLock::new(VerifyQueue::new(config.max_tx_verify_cycles)));

    let service = TxPoolService {
        tx_pool: Arc::new(RwLock::new(tx_pool)),
        orphan: Arc::new(RwLock::new(OrphanPool::new())),
        consensus: Arc::clone(&shared.snapshot.cloned_consensus()),
        tx_pool_config: Arc::new(config.clone()),
        block_assembler: None,
        // Temporary service for cycle measurement. Its verify cache is
        // independent from the benchmark controller's cache (see start_controller).
        // When this service is dropped via signal.cancel(), its cache is discarded.
        txs_verify_cache: Arc::new(RwLock::new(init_cache())),
        callbacks: Arc::new(crate::callback::Callbacks::new()),
        network: shared.network.clone(),
        tx_relay_sender,
        resolve_queue: Arc::clone(&resolve_queue),
        ordered_resolve_queue: Arc::clone(&ordered_resolve_queue),
        verify_queue: Arc::clone(&verify_queue),
        block_assembler_sender,
        fee_estimator: FeeEstimator::new_dummy(),
    };

    let signal = ckb_stop_handler::CancellationToken::new();
    let (chunk_tx, chunk_rx) = watch::channel(ChunkCommand::Resume);

    let mut verify_mgr =
        crate::verify_mgr::VerifyMgr::new(service.clone(), chunk_rx, signal.child_token());
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

    (service, signal)
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

async fn wait_for_pending(controller: &crate::TxPoolController, count: usize) {
    let start = std::time::Instant::now();
    let mut last_log = start;
    let result = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let info = controller.get_tx_pool_info().ok();
            let pending = info.as_ref().map(|i| i.pending_size).unwrap_or(0);
            if pending >= count {
                break;
            }
            let now = std::time::Instant::now();
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
            tokio::time::sleep(Duration::from_millis(50)).await;
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

const SIZES: &[usize] = &[50, 100];
const PEER_COUNTS: &[usize] = &[1, 2, 4];
const WORKER_COUNTS: &[usize] = &[4, 8, 12];
const WARM_POOL_SIZE: usize = 100;
const DEPENDENT_SIZES: &[usize] = &[10, 20];
const DEPENDENT_WARM_POOL_SIZE: usize = 10;

// Quick matrix: runs in about 5 minutes and is used when QUICK_BENCH is set.
const QUICK_SIZES: &[usize] = &[50];
const QUICK_PEER_COUNTS: &[usize] = &[1];
const QUICK_WORKER_COUNTS: &[usize] = &[8];
const QUICK_WARM_POOL_SIZE: usize = 50;
const QUICK_DEPENDENT_SIZES: &[usize] = &[10];
const QUICK_DEPENDENT_WARM_POOL_SIZE: usize = 10;

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
                let cycle = measure_sample_cycle(&shared);
                let cycles = txs.iter().map(|tx| (tx.hash(), cycle)).collect();
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
                let cycles = measure_cycles(&shared, &txs, false);
                (shared, txs, cycles)
            }
            TxType::DependentAlwaysSuccess => {
                let shared = SharedBench::new_always_success(issue_outputs);
                let txs = build_dependent_chain(&shared, issue_outputs);
                let cycles = measure_cycles(&shared, &txs, true);
                (shared, txs, cycles)
            }
            TxType::DependentSecp => {
                let (shared, _) = SharedBench::new_secp(issue_outputs);
                let txs = build_dependent_chain(&shared, issue_outputs);
                let cycles = measure_cycles(&shared, &txs, true);
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

    fn warm(&self) -> (Vec<TransactionView>, Vec<u64>) {
        let txs = self.txs[..self.warm_pool_size].to_vec();
        let cycles = txs
            .iter()
            .map(|tx| *self.cycles.get(&tx.hash()).expect("missing cycle"))
            .collect();
        (txs, cycles)
    }

    fn target(&self, size: usize) -> (Vec<TransactionView>, Vec<u64>) {
        let end = self.warm_pool_size + size;
        let txs = self.txs[self.warm_pool_size..end].to_vec();
        let cycles = txs
            .iter()
            .map(|tx| *self.cycles.get(&tx.hash()).expect("missing cycle"))
            .collect();
        (txs, cycles)
    }
}

fn measure_sample_cycle(shared: &SharedBench) -> u64 {
    let out_point = shared.issue_out_points(1).pop().expect("one output");
    let sample = build_tx(&out_point, 4_000);
    let (service, signal) = shared.runtime.block_on(async { start_service(shared, 8) });
    let cycle = shared.runtime.block_on(async {
        service
            ._test_accept_tx(sample)
            .await
            .expect("measure cycle")
            .cycles
    });
    signal.cancel();
    cycle
}

fn measure_cycles(
    shared: &SharedBench,
    txs: &[TransactionView],
    use_process: bool,
) -> HashMap<ckb_types::packed::Byte32, u64> {
    let (service, signal) = shared.runtime.block_on(async { start_service(shared, 8) });
    let cycles = shared.runtime.block_on(async {
        let mut cycles = HashMap::with_capacity(txs.len());
        for tx in txs {
            let c = if use_process {
                service
                    .process_tx(tx.clone(), None)
                    .await
                    .expect("measure cycles via process_tx")
                    .cycles
            } else {
                service
                    ._test_accept_tx(tx.clone())
                    .await
                    .expect("measure cycles via _test_accept_tx")
                    .cycles
            };
            cycles.insert(tx.hash(), c);
        }
        cycles
    });
    signal.cancel();
    cycles
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

fn chunk_vec<T>(mut vec: Vec<T>, n: usize) -> Vec<Vec<T>> {
    if n == 0 {
        return vec![vec];
    }
    let chunk_size = vec.len().div_ceil(n);
    let mut chunks = Vec::new();
    while !vec.is_empty() {
        let split_at = std::cmp::min(chunk_size, vec.len());
        let tail = vec.split_off(split_at);
        chunks.push(vec);
        vec = tail;
    }
    chunks
}

fn submit_and_wait(
    runtime: &tokio::runtime::Runtime,
    controller: &crate::TxPoolController,
    txs: Vec<TransactionView>,
    cycles: Vec<u64>,
    target_pending: usize,
    submitters: usize,
) {
    runtime.block_on(async {
        // `submitters` concurrent submitters mimic several peers feeding the service actor.
        // The actor itself still processes messages one by one.
        let tx_chunks = chunk_vec(txs, submitters);
        let c_chunks = chunk_vec(cycles, submitters);
        let controller_for_wait = controller.clone();
        let mut handles = Vec::with_capacity(tx_chunks.len());
        for (tx_chunk, c_chunk) in tx_chunks.into_iter().zip(c_chunks) {
            let controller = controller.clone();
            handles.push(tokio::spawn(async move {
                for (tx, c) in tx_chunk.into_iter().zip(c_chunk) {
                    controller
                        .submit_remote_tx(tx, c, 1.into())
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

fn register_cold_bench(
    group: &mut BenchmarkGroup<'_, criterion::measurement::WallTime>,
    mode: &str,
    data: &BenchData,
    peers: usize,
    workers: usize,
    size: usize,
) {
    let (mut txs, mut cycles) = data.target(size);
    // Submit dependent chains in reverse order so children land in the orphan pool
    // and are recovered after their parents are accepted. Submitting in natural
    // order would route them to the ordered resolve queue, which is not re-driven
    // once the parent leaves the pipeline.
    if data.tx_type.is_dependent() {
        txs.reverse();
        cycles.reverse();
    }
    let tx_type = data.tx_type.as_str();
    group.throughput(Throughput::Elements(size as u64));

    if data.tx_type.is_dependent() {
        // Cold dependent txs depend on the warm prefix of the chain. Pre-submit that
        // prefix (not measured) so the reversed target txs have parents in the pool.
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
                            warm_txs.clone(),
                            warm_cycles.clone(),
                            data.warm_pool_size,
                            1,
                        );
                        handle
                    },
                    |handle| {
                        submit_and_wait(
                            &data.shared.runtime,
                            &handle.controller,
                            txs.clone(),
                            cycles.clone(),
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
                            txs.clone(),
                            cycles.clone(),
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
    // entries. The measured closure then submits target_txs (different hashes),
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
                        warm_txs.clone(),
                        warm_cycles.clone(),
                        data.warm_pool_size,
                        peers,
                    );
                    handle
                },
                |handle| {
                    submit_and_wait(
                        &data.shared.runtime,
                        &handle.controller,
                        target_txs.clone(),
                        target_cycles.clone(),
                        expected_pending,
                        peers,
                    )
                },
                BatchSize::PerIteration,
            )
        },
    );
}

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
    } else {
        BenchMatrix {
            sizes: SIZES,
            peer_counts: PEER_COUNTS,
            worker_counts: WORKER_COUNTS,
            warm_pool_size: WARM_POOL_SIZE,
            dependent_sizes: DEPENDENT_SIZES,
            dependent_warm_pool_size: DEPENDENT_WARM_POOL_SIZE,
        }
    }
}

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

    for workers in matrix.worker_counts {
        for peers in matrix.peer_counts {
            for (data, sizes) in &data_sets {
                for size in *sizes {
                    register_cold_bench(&mut group, mode, data, *peers, *workers, *size);
                }
            }
        }
    }

    for workers in matrix.worker_counts {
        for peers in matrix.peer_counts {
            for (data, sizes) in &data_sets {
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
        let cycles = measure_cycles(&shared, &txs, true);
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
