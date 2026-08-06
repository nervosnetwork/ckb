use ckb_app_config::{NetworkConfig, TxPoolConfig};
use ckb_async_runtime::{Handle, new_global_runtime};
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_crypto::secp::Privkey;
use ckb_dao_utils::genesis_dao_data;
use ckb_fee_estimator::FeeEstimator;
use ckb_network::{Flags, NetworkController, NetworkService, NetworkState, network::TransportType};
use ckb_snapshot::Snapshot;
use ckb_store::attach_block_cell;
use ckb_system_scripts::BUNDLED_CELL;
use ckb_test_chain_utils::{MockStore, always_success_cell};
use ckb_tx_pool::{TxPoolController, TxPoolServiceBuilder};
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
use std::{
    error::Error,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tempfile::TempDir;
use tokio::sync::{Barrier, Notify, RwLock};

const MAX_TX_VERIFY_CYCLES: u64 = 70_000_000;
const ISSUE_CAPACITY_BYTES: usize = 500_000;
const SECP_PRIVKEY: H256 =
    h256!("0xb2b3324cece882bca684eaf202667bb56ed8e8c2fd4b4dc71f615ebd6d9055a5");
const SECP_PUBKEY_HASH: H160 = h160!("0x779e5930892a0a9bf2fedfe048f685466c7d0396");
const SECP_ISSUE_CAPACITY: u64 = 10_000_000 * 100_000_000;
const SECP_FEE: u64 = 1_000 * 100_000_000;

#[derive(Default)]
struct Completion {
    accepted: AtomicUsize,
    changed: Notify,
}

impl Completion {
    fn record(&self) {
        self.accepted.fetch_add(1, Ordering::Release);
        self.changed.notify_one();
    }

    async fn wait_for(&self, target: usize) -> Result<(), Box<dyn Error>> {
        tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                if self.accepted.load(Ordering::Acquire) >= target {
                    break;
                }
                self.changed.notified().await;
            }
        })
        .await
        .map_err(|_| {
            std::io::Error::other(format!(
                "timed out after accepting {}/{} transactions",
                self.accepted.load(Ordering::Acquire),
                target
            ))
        })?;
        Ok(())
    }
}

fn tx_pool_config(workers: usize, enable_rbf: bool) -> TxPoolConfig {
    TxPoolConfig {
        max_tx_pool_size: 180_000_000,
        min_fee_rate: FeeRate::zero(),
        min_rbf_rate: if enable_rbf {
            FeeRate::from_u64(1_000)
        } else {
            FeeRate::zero()
        },
        max_tx_verify_cycles: MAX_TX_VERIFY_CYCLES,
        max_tx_verify_workers: workers,
        max_ancestors_count: 1_000,
        keep_rejected_tx_hashes_days: 1,
        keep_rejected_tx_hashes_count: 100_000,
        persisted_data: Default::default(),
        recent_reject: Default::default(),
        expiry_hours: 24,
        ..Default::default()
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
        .output_data(always_success_data.clone())
        .witness(always_success_script.clone().into_witness())
        .build();
    let issue_output = CellOutput::new_builder()
        .capacity(Capacity::bytes(ISSUE_CAPACITY_BYTES).expect("valid issue capacity"))
        .lock(always_success_script.clone())
        .build();
    let issue_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .outputs((0..issue_outputs).map(|_| issue_output.clone()))
        .outputs_data((0..issue_outputs).map(|_| Bytes::default().pack()))
        .build();
    let dao = genesis_dao_data(vec![&always_success_tx, &issue_tx]).expect("valid genesis DAO");
    let genesis = BlockBuilder::default()
        .timestamp(1_557_310_743u64)
        .compact_target(difficulty_to_compact(U256::from(1_000u64)))
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
        let previous_epoch_hash = epoch_ext.last_block_hash_in_previous_epoch();
        db_txn.insert_block(genesis).expect("insert genesis block");
        db_txn.attach_block(genesis).expect("attach genesis block");
        attach_block_cell(&db_txn, genesis).expect("attach genesis cells");
        db_txn
            .insert_block_epoch_index(&genesis.hash(), &previous_epoch_hash)
            .expect("insert genesis epoch index");
        db_txn
            .insert_epoch_ext(&previous_epoch_hash, &epoch_ext)
            .expect("insert genesis epoch extension");
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
            .expect("insert genesis block extension");
        db_txn.commit().expect("commit genesis snapshot");
    }
    let snapshot = Arc::new(Snapshot::new(
        genesis.header(),
        U256::zero(),
        epoch_ext,
        store.store().get_snapshot(),
        Default::default(),
        consensus,
    ));
    (store, snapshot)
}

fn start_network(
    consensus: &Consensus,
    handle: &Handle,
) -> Result<(TempDir, NetworkController), Box<dyn Error>> {
    let directory = TempDir::new()?;
    let state = Arc::new(
        NetworkState::from_config(NetworkConfig {
            max_peers: 19,
            max_outbound_peers: 5,
            path: directory.path().to_path_buf(),
            ping_interval_secs: 15,
            ping_timeout_secs: 20,
            connect_outbound_interval_secs: 1,
            discovery_local_address: true,
            bootnode_mode: true,
            reuse_port_on_linux: true,
            ..Default::default()
        })
        .map_err(|error| std::io::Error::other(error.to_string()))?,
    );
    let controller = NetworkService::new(
        state,
        vec![],
        vec![],
        (
            consensus.identify_name(),
            "tx-pool-bench".to_owned(),
            Flags::all(),
        ),
        TransportType::Tcp,
    )
    .start(handle)
    .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok((directory, controller))
}

fn build_tx(input: OutPoint) -> TransactionView {
    TransactionBuilder::default()
        .cell_dep(always_success_dep())
        .input(CellInput::new(input, 0))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(100).expect("valid output capacity"))
                .lock(always_success_script())
                .build(),
        )
        .output_data(Bytes::default().pack())
        .build()
}

fn build_multi_input_tx(inputs: impl IntoIterator<Item = OutPoint>) -> TransactionView {
    TransactionBuilder::default()
        .cell_dep(always_success_dep())
        .inputs(inputs.into_iter().map(|input| CellInput::new(input, 0)))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(100).expect("valid output capacity"))
                .lock(always_success_script())
                .build(),
        )
        .output_data(Bytes::default().pack())
        .build()
}

fn build_fanout_parent(input: OutPoint, output_count: usize) -> TransactionView {
    let output = CellOutput::new_builder()
        .capacity(Capacity::bytes(100).expect("valid output capacity"))
        .lock(always_success_script())
        .build();
    TransactionBuilder::default()
        .cell_dep(always_success_dep())
        .input(CellInput::new(input, 0))
        .outputs((0..output_count).map(|_| output.clone()))
        .outputs_data((0..output_count).map(|_| Bytes::default().pack()))
        .build()
}

fn secp_script() -> Script {
    let data: Bytes = BUNDLED_CELL
        .get("specs/cells/secp256k1_blake160_sighash_all")
        .expect("load secp256k1_blake160_sighash_all")
        .to_vec()
        .into();
    Script::new_builder()
        .code_hash(CellOutput::calc_data_hash(&data))
        .args(Bytes::from(SECP_PUBKEY_HASH.as_bytes()))
        .hash_type(ckb_types::core::ScriptHashType::Data)
        .build()
}

fn bundled_cell(key: &str) -> (CellOutput, Bytes) {
    let data: Bytes = BUNDLED_CELL
        .get(key)
        .expect("load bundled cell")
        .to_vec()
        .into();
    let cell = CellOutput::new_builder()
        .capacity(Capacity::bytes(data.len()).expect("valid bundled cell capacity"))
        .build();
    (cell, data)
}

fn secp_test_consensus(issue_outputs: usize) -> (Consensus, TransactionView, Vec<CellDep>) {
    let (code_cell, code_data) = bundled_cell("specs/cells/secp256k1_blake160_sighash_all");
    let (data_cell, data_data) = bundled_cell("specs/cells/secp256k1_data");
    let system_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .outputs(vec![code_cell, data_cell])
        .outputs_data(vec![code_data.pack(), data_data.pack()])
        .witness(secp_script().into_witness())
        .build();
    let cell_deps = vec![
        CellDep::new_builder()
            .out_point(OutPoint::new(system_tx.hash(), 0))
            .build(),
        CellDep::new_builder()
            .out_point(OutPoint::new(system_tx.hash(), 1))
            .build(),
    ];
    let issue_output = CellOutput::new_builder()
        .capacity(Capacity::shannons(SECP_ISSUE_CAPACITY))
        .lock(secp_script())
        .build();
    let issue_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .outputs((0..issue_outputs).map(|_| issue_output.clone()))
        .outputs_data((0..issue_outputs).map(|_| Bytes::default().pack()))
        .build();
    let dao = genesis_dao_data(vec![&system_tx, &issue_tx]).expect("valid secp genesis DAO");
    let genesis = BlockBuilder::default()
        .timestamp(1_557_310_743u64)
        .compact_target(difficulty_to_compact(U256::from(1_000u64)))
        .dao(dao)
        .transaction(system_tx)
        .transaction(issue_tx.clone())
        .build();
    let consensus = ConsensusBuilder::default()
        .genesis_block(genesis)
        .cellbase_maturity(EpochNumberWithFraction::new(0, 0, 1))
        .build();
    (consensus, issue_tx, cell_deps)
}

fn build_secp_tx(input: OutPoint, cell_deps: &[CellDep], output_capacity: u64) -> TransactionView {
    let raw = TransactionBuilder::default()
        .input(CellInput::new(input, 0))
        .cell_deps(cell_deps.iter().cloned())
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::shannons(output_capacity))
                .lock(secp_script())
                .build(),
        )
        .output_data(Bytes::default().pack())
        .build();
    let witness_placeholder = WitnessArgs::new_builder()
        .lock(Some(Bytes::from(vec![0u8; 65])))
        .build();
    let witness_len = witness_placeholder.as_bytes().len() as u64;
    let mut blake2b = ckb_hash::new_blake2b();
    let mut message = [0u8; 32];
    blake2b.update(&raw.hash().raw_data()[..]);
    blake2b.update(&witness_len.to_le_bytes());
    blake2b.update(&witness_placeholder.as_bytes());
    blake2b.finalize(&mut message);
    let private_key: Privkey = SECP_PRIVKEY.into();
    let message = H256::from(message);
    let signature: Bytes = private_key
        .sign_recoverable(&message)
        .expect("sign secp transaction")
        .serialize()
        .into();
    let witness = witness_placeholder
        .as_builder()
        .lock(Some(signature))
        .build();
    raw.as_advanced_builder()
        .set_witnesses(vec![witness.as_bytes().into()])
        .build()
}

fn build_workload(
    scenario: &str,
    transaction_count: usize,
) -> Result<(Consensus, Vec<TransactionView>), Box<dyn Error>> {
    if let Some(depth) = scenario.strip_prefix("dependent_forest_") {
        let depth: usize = depth.parse()?;
        if depth == 0 {
            return Err(std::io::Error::other("dependency depth must be non-zero").into());
        }
        let chain_count = transaction_count.div_ceil(depth);
        let (consensus, issue_tx) = test_consensus(chain_count);
        let mut transactions = Vec::with_capacity(transaction_count);
        for chain_index in 0..chain_count {
            let output_index = u32::try_from(chain_index)
                .map_err(|_| std::io::Error::other("dependency-forest index overflow"))?;
            let mut input = OutPoint::new(issue_tx.hash(), output_index);
            for _ in 0..depth {
                if transactions.len() == transaction_count {
                    break;
                }
                let transaction = build_tx(input);
                input = OutPoint::new(transaction.hash(), 0);
                transactions.push(transaction);
            }
        }
        return Ok((consensus, transactions));
    }
    if let Some(fan_in) = scenario.strip_prefix("always_success_fanin_") {
        let fan_in: usize = fan_in.parse()?;
        if fan_in == 0 {
            return Err(std::io::Error::other("fan-in must be non-zero").into());
        }
        let issue_outputs = transaction_count
            .checked_mul(fan_in)
            .ok_or_else(|| std::io::Error::other("fan-in workload size overflow"))?;
        let (consensus, issue_tx) = test_consensus(issue_outputs);
        let transactions = (0..transaction_count)
            .map(|transaction_index| {
                let first = transaction_index * fan_in;
                build_multi_input_tx((first..first + fan_in).map(|index| {
                    OutPoint::new(
                        issue_tx.hash(),
                        u32::try_from(index).expect("bounded index"),
                    )
                }))
            })
            .collect();
        return Ok((consensus, transactions));
    }
    match scenario {
        "always_success" => {
            let (consensus, issue_tx) = test_consensus(transaction_count);
            let transactions = (0..transaction_count)
                .map(|index| build_tx(OutPoint::new(issue_tx.hash(), index as u32)))
                .collect();
            Ok((consensus, transactions))
        }
        "secp256k1" => {
            let (consensus, issue_tx, cell_deps) = secp_test_consensus(transaction_count);
            let transactions = (0..transaction_count)
                .map(|index| {
                    build_secp_tx(
                        OutPoint::new(issue_tx.hash(), index as u32),
                        &cell_deps,
                        SECP_ISSUE_CAPACITY - SECP_FEE,
                    )
                })
                .collect();
            Ok((consensus, transactions))
        }
        "dependent" => {
            let (consensus, issue_tx) = test_consensus(1);
            let mut transactions = Vec::with_capacity(transaction_count);
            let mut input = OutPoint::new(issue_tx.hash(), 0);
            for _ in 0..transaction_count {
                let transaction = build_tx(input);
                input = OutPoint::new(transaction.hash(), 0);
                transactions.push(transaction);
            }
            Ok((consensus, transactions))
        }
        "dependent_reverse" => {
            let (consensus, issue_tx) = test_consensus(1);
            let mut transactions = Vec::with_capacity(transaction_count);
            let mut input = OutPoint::new(issue_tx.hash(), 0);
            for _ in 0..transaction_count {
                let transaction = build_tx(input);
                input = OutPoint::new(transaction.hash(), 0);
                transactions.push(transaction);
            }
            transactions.reverse();
            Ok((consensus, transactions))
        }
        "fanout" | "fanout_reverse" => {
            let child_count = transaction_count.saturating_sub(1);
            let (consensus, issue_tx) = test_consensus(1);
            let parent = build_fanout_parent(OutPoint::new(issue_tx.hash(), 0), child_count);
            let mut children = (0..child_count)
                .map(|index| build_tx(OutPoint::new(parent.hash(), index as u32)))
                .collect::<Vec<_>>();
            let transactions = if scenario == "fanout_reverse" {
                children.reverse();
                children.push(parent);
                children
            } else {
                let mut transactions = Vec::with_capacity(transaction_count);
                transactions.push(parent);
                transactions.extend(children);
                transactions
            };
            Ok((consensus, transactions))
        }
        _ => Err(std::io::Error::other(format!("unknown scenario: {scenario}")).into()),
    }
}

async fn submit_batch(
    controller: &TxPoolController,
    completion: &Completion,
    transactions: Arc<Vec<TransactionView>>,
    cycles: Arc<Vec<u64>>,
    peers: usize,
    expected_total: usize,
) -> Result<(), Box<dyn Error>> {
    let chunk_size = transactions.len().div_ceil(peers.max(1));
    let ranges: Vec<_> = (0..transactions.len())
        .step_by(chunk_size.max(1))
        .map(|start| (start, (start + chunk_size).min(transactions.len())))
        .collect();
    let barrier = Arc::new(Barrier::new(ranges.len() + 1));
    let mut submissions = Vec::with_capacity(ranges.len());
    for (peer, (start, end)) in ranges.into_iter().enumerate() {
        let controller = controller.clone();
        let transactions = Arc::clone(&transactions);
        let cycles = Arc::clone(&cycles);
        let barrier = Arc::clone(&barrier);
        submissions.push(tokio::spawn(async move {
            barrier.wait().await;
            for index in start..end {
                controller
                    .submit_remote_tx(
                        transactions[index].clone(),
                        cycles[index],
                        (peer + 1).into(),
                    )
                    .await
                    .map_err(|error| std::io::Error::other(error.to_string()))?;
            }
            Ok::<(), std::io::Error>(())
        }));
    }
    barrier.wait().await;
    for submission in submissions {
        submission.await??;
    }
    completion.wait_for(expected_total).await
}

async fn submit_dependency_forest(
    controller: &TxPoolController,
    completion: &Completion,
    transactions: &[TransactionView],
    cycles: &[u64],
    depth: usize,
    peers: usize,
    mut expected_total: usize,
) -> Result<(), Box<dyn Error>> {
    if depth == 0 || !transactions.len().is_multiple_of(depth) {
        return Err(std::io::Error::other(
            "dependency-forest batch must contain complete non-empty chains",
        )
        .into());
    }
    let chain_count = transactions.len() / depth;
    for level in 0..depth {
        let indexes = (0..chain_count).map(|chain| chain * depth + level);
        let layer_transactions = indexes
            .clone()
            .map(|index| transactions[index].clone())
            .collect();
        let layer_cycles = indexes.map(|index| cycles[index]).collect();
        expected_total += chain_count;
        submit_batch(
            controller,
            completion,
            Arc::new(layer_transactions),
            Arc::new(layer_cycles),
            peers,
            expected_total,
        )
        .await?;
    }
    Ok(())
}

fn parse_arg(index: usize, default: usize) -> Result<usize, Box<dyn Error>> {
    match std::env::args().nth(index) {
        Some(value) => Ok(value.parse()?),
        None => Ok(default),
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let scenario = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "always_success".to_owned());
    let callback_delay_us = scenario
        .strip_prefix("always_success_callback_")
        .and_then(|value| value.strip_suffix("us"))
        .map(str::parse::<u64>)
        .transpose()?;
    let workload_scenario = if callback_delay_us.is_some() {
        "always_success"
    } else {
        scenario.as_str()
    };
    let target_count = parse_arg(2, 1_000)?;
    let warm_count = parse_arg(3, 100)?;
    let workers = parse_arg(4, 8)?;
    let peers = parse_arg(5, 8)?;
    let runtime_threads = std::thread::available_parallelism().map_or(8, |count| count.get());
    let (handle, _handle_stop, runtime) = new_global_runtime(Some(runtime_threads));
    let (consensus, transactions) = build_workload(workload_scenario, target_count + warm_count)?;
    let consensus = Arc::new(consensus);
    let (_store, snapshot) = snapshot_with_genesis(Arc::clone(&consensus));
    let (_network_directory, network) = start_network(&consensus, &handle)?;
    #[cfg(feature = "cross-version-legacy-bench-adapter")]
    let (mut builder, controller, _relay_guard) = {
        let (relay_sender, relay_receiver) = ckb_channel::bounded(1024);
        let (builder, controller) = TxPoolServiceBuilder::new(
            tx_pool_config(workers, false),
            Arc::clone(&snapshot),
            None,
            Arc::new(RwLock::new(init_cache())),
            &handle,
            relay_sender,
            FeeEstimator::new_dummy(),
        );
        (builder, controller, relay_receiver)
    };
    #[cfg(not(feature = "cross-version-legacy-bench-adapter"))]
    let (mut builder, controller, _relay_guard) = {
        let (builder, controller, relay_receiver) = TxPoolServiceBuilder::new(
            tx_pool_config(workers, false),
            Arc::clone(&snapshot),
            None,
            Arc::new(RwLock::new(init_cache())),
            &handle,
            FeeEstimator::new_dummy(),
        )
        .map_err(|error| std::io::Error::other(error.to_string()))?;
        (builder, controller, relay_receiver)
    };
    let completion = Arc::new(Completion::default());
    let callback_completion = Arc::clone(&completion);
    builder.register_pending(Box::new(move |_| {
        if let Some(delay) = callback_delay_us {
            std::thread::sleep(Duration::from_micros(delay));
        }
        callback_completion.record();
    }));
    builder.start(network);
    controller
        .get_tx_pool_info()
        .map_err(|error| std::io::Error::other(error.to_string()))?;

    let sample_cycles = |transaction: &TransactionView| {
        controller
            .test_accept_tx(transaction.clone())
            .map_err(|error| std::io::Error::other(error.to_string()))?
            .map_err(|error| std::io::Error::other(error.to_string()))
            .map(|completed| completed.cycles)
    };
    let cycles = if workload_scenario == "secp256k1" {
        transactions
            .iter()
            .map(sample_cycles)
            .collect::<Result<Vec<_>, _>>()?
    } else {
        let sample = if workload_scenario.ends_with("_reverse") {
            transactions.last()
        } else {
            transactions.first()
        }
        .ok_or_else(|| std::io::Error::other("benchmark workload must not be empty"))?;
        vec![sample_cycles(sample)?; transactions.len()]
    };

    let warm = Arc::new(transactions[..warm_count].to_vec());
    let target = Arc::new(transactions[warm_count..].to_vec());
    let warm_cycles = Arc::new(cycles[..warm_count].to_vec());
    let target_cycles = Arc::new(cycles[warm_count..].to_vec());

    let dependency_depth = workload_scenario
        .strip_prefix("dependent_forest_")
        .map(str::parse::<usize>)
        .transpose()?;
    if let Some(depth) = dependency_depth {
        runtime.block_on(submit_dependency_forest(
            &controller,
            &completion,
            warm.as_slice(),
            warm_cycles.as_slice(),
            depth,
            peers,
            0,
        ))?;
    } else {
        runtime.block_on(submit_batch(
            &controller,
            &completion,
            warm,
            warm_cycles,
            peers,
            warm_count,
        ))?;
    }
    let profile_started_unix_ns = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let started = Instant::now();
    if let Some(depth) = dependency_depth {
        runtime.block_on(submit_dependency_forest(
            &controller,
            &completion,
            target.as_slice(),
            target_cycles.as_slice(),
            depth,
            peers,
            warm_count,
        ))?;
    } else {
        runtime.block_on(submit_batch(
            &controller,
            &completion,
            target,
            target_cycles,
            peers,
            warm_count + target_count,
        ))?;
    }
    let elapsed = started.elapsed();
    let profile_ended_unix_ns = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let throughput = target_count as f64 / elapsed.as_secs_f64();
    println!(
        "BENCH_RESULT scenario={scenario} target={target_count} warm={warm_count} workers={workers} peers={peers} elapsed_ns={} throughput_tps={throughput:.3} accepted={}",
        elapsed.as_nanos(),
        completion.accepted.load(Ordering::Acquire)
    );
    println!(
        "PROFILE_WINDOW start_unix_ns={profile_started_unix_ns} end_unix_ns={profile_ended_unix_ns}"
    );
    Ok(())
}
