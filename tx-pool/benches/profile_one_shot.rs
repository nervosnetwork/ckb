//! One-shot, fixed-workload tx-pool profiling harness.

mod allocation_observation;
use allocation_observation::{begin_allocation_window, end_allocation_window};

#[cfg(all(feature = "tokio-trace", not(tokio_unstable)))]
compile_error!("the `tokio-trace` benchmark requires RUSTFLAGS=\"--cfg tokio_unstable\"");

use ckb_app_config::{NetworkConfig, TxPoolConfig};
use ckb_async_runtime::{Handle, new_global_runtime};
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_crypto::secp::Privkey;
use ckb_dao_utils::genesis_dao_data;
use ckb_fee_estimator::FeeEstimator;
use ckb_network::{
    Flags, NetworkController, NetworkService, NetworkState, PeerIndex, network::TransportType,
};
use ckb_proposal_table::ProposalView;
use ckb_script::{TransactionScriptsVerifier, TxVerifyEnv};
use ckb_snapshot::Snapshot;
#[cfg(feature = "cross-version-legacy-bench-adapter")]
use ckb_stop_handler::broadcast_exit_signals;
use ckb_store::{attach_block_cell, data_loader_wrapper::AsDataLoader};
use ckb_system_scripts::BUNDLED_CELL;
use ckb_test_chain_utils::{MockStore, always_success_cell};
#[cfg(not(feature = "cross-version-legacy-bench-adapter"))]
use ckb_tx_pool::service::TxVerificationResultReceiver;
use ckb_tx_pool::{TxPoolController, TxPoolServiceBuilder, service::TxVerificationResult};
use ckb_types::{
    H160, H256, U256,
    bytes::Bytes,
    core::{
        BlockBuilder, BlockExt, Capacity, EpochNumberWithFraction, FeeRate, TransactionBuilder,
        TransactionView, cell::resolve_transaction, tx_pool::TRANSACTION_SIZE_LIMIT,
    },
    h160, h256,
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint, Script, WitnessArgs},
    prelude::*,
    utilities::difficulty_to_compact,
};
use ckb_verification::cache::init_cache;
#[cfg(feature = "cross-version-legacy-bench-adapter")]
use std::io::Write;
#[cfg(unix)]
use std::mem::MaybeUninit;
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    error::Error,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tempfile::TempDir;
#[cfg(feature = "cross-version-legacy-bench-adapter")]
use tokio::sync::Barrier;
use tokio::sync::{Notify, RwLock};

const MAX_TX_VERIFY_CYCLES: u64 = 70_000_000;
const ISSUE_CAPACITY_BYTES: usize = 500_000;
const FANOUT_OUTPUT_CAPACITY_BYTES: usize = 100;
const FANOUT_COHORT_SIZE: usize = fanout_readiness::CHILDREN + 1;
const COMPLETION_COUNTER_SHARDS: usize = 64;
const SECP_PRIVKEY: H256 =
    h256!("0xb2b3324cece882bca684eaf202667bb56ed8e8c2fd4b4dc71f615ebd6d9055a5");
const SECP_PUBKEY_HASH: H160 = h160!("0x779e5930892a0a9bf2fedfe048f685466c7d0396");
const SECP_ISSUE_CAPACITY: u64 = 10_000_000 * 100_000_000;
const SECP_FEE: u64 = 1_000 * 100_000_000;
type RelayOk = (Byte32, Option<PeerIndex>);
type RelayOkSet = HashSet<RelayOk>;
type RelayRejectSet = HashSet<Byte32>;
type BenchResult<T> = Result<T, Box<dyn Error>>;

fn bench_error(message: impl ToString) -> Box<dyn Error> {
    std::io::Error::other(message.to_string()).into()
}

fn require(condition: bool, message: impl ToString) -> BenchResult<()> {
    condition.then_some(()).ok_or_else(|| bench_error(message))
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().expect("benchmark observer lock poisoned")
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn finish_blake2b(hasher: ckb_hash::Blake2b) -> String {
    let mut digest = [0u8; 32];
    hasher.finalize(&mut digest);
    hex_bytes(&digest)
}

fn corpus_observation(
    consensus: &Consensus,
    transactions: &[TransactionView],
    cycles: &[u64],
    script_preflight_count: usize,
) -> BenchResult<serde_json::Value> {
    require(
        transactions.len() == cycles.len(),
        "corpus transaction/cycle length differs",
    )?;
    let mut transaction_bytes = ckb_hash::new_blake2b();
    let mut transaction_hashes = ckb_hash::new_blake2b();
    let mut cycle_values = ckb_hash::new_blake2b();
    let mut cycle_sum = 0u64;
    for (transaction, cycle) in transactions.iter().zip(cycles) {
        transaction_bytes.update(transaction.data().as_slice());
        transaction_hashes.update(transaction.hash().as_slice());
        cycle_values.update(&cycle.to_le_bytes());
        cycle_sum = cycle_sum
            .checked_add(*cycle)
            .ok_or_else(|| bench_error("corpus cycle sum overflow"))?;
    }
    let consensus_descriptor = format!(
        "id={};genesis={};max_block_cycles={};tx_version={};hardfork={:?}",
        consensus.id,
        hex_bytes(consensus.genesis_hash.as_slice()),
        consensus.max_block_cycles,
        consensus.tx_version,
        consensus.hardfork_switch(),
    );
    Ok(serde_json::json!({
        "transaction_count": transactions.len(),
        "transaction_bytes_blake2b": finish_blake2b(transaction_bytes),
        "transaction_hashes_blake2b": finish_blake2b(transaction_hashes),
        "cycles_blake2b": finish_blake2b(cycle_values),
        "cycles_sum": cycle_sum,
        "cycle_assignment_count": cycles.len(),
        "script_preflight_count": script_preflight_count,
        "consensus_blake2b": hex_bytes(&ckb_hash::blake2b_256(consensus_descriptor.as_bytes())),
    }))
}

#[cfg(unix)]
fn process_cpu_nanos() -> BenchResult<(u64, u64)> {
    let mut usage = MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: `getrusage` initializes the complete `rusage` value on success.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: the successful call above initialized `usage`.
    let usage = unsafe { usage.assume_init() };
    let timeval_nanos = |value: libc::timeval| -> BenchResult<u64> {
        let seconds = u64::try_from(value.tv_sec)?;
        let micros = u64::try_from(value.tv_usec)?;
        require(
            micros < 1_000_000,
            "getrusage returned invalid microseconds",
        )?;
        seconds
            .checked_mul(1_000_000_000)
            .and_then(|nanos| nanos.checked_add(micros * 1_000))
            .ok_or_else(|| bench_error("process CPU time overflow"))
    };
    Ok((
        timeval_nanos(usage.ru_utime)?,
        timeval_nanos(usage.ru_stime)?,
    ))
}

#[cfg(not(unix))]
fn process_cpu_nanos() -> BenchResult<(u64, u64)> {
    Err(bench_error(
        "target-window process CPU measurement requires Unix",
    ))
}

#[cfg(feature = "profiling")]
mod profile_spans;
#[cfg(feature = "profiling")]
use profile_spans::ProfileSpanRecorder;

#[cfg(feature = "tokio-trace")]
struct ConsoleGuard {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<Result<(), String>>>,
}

#[cfg(feature = "tokio-trace")]
impl ConsoleGuard {
    fn start(
        server: console_subscriber::Server,
        listener: std::net::TcpListener,
    ) -> Result<Self, String> {
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let thread = std::thread::Builder::new()
            .name("bench-console".into())
            .spawn(move || {
                // Observing the observer's own runtime creates unrelated work
                // and can feed its events back into the aggregation loop.
                let _subscriber =
                    tracing::subscriber::set_default(tracing::subscriber::NoSubscriber::default());
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| error.to_string())?;
                runtime.block_on(async {
                    let listener = tokio::net::TcpListener::from_std(listener)
                        .map_err(|error| error.to_string())?;
                    let incoming = futures_util::stream::unfold(listener, |listener| async {
                        Some((listener.accept().await.map(|(stream, _)| stream), listener))
                    });
                    let console_subscriber::ServerParts {
                        instrument_server,
                        aggregator,
                        ..
                    } = server.into_parts();
                    // Console drains via poll_recv with a noop waker. A
                    // cooperative-budget Pending would look like an empty
                    // queue and strand events until a later timer tick.
                    // This finite diagnostic has a private observer runtime.
                    let mut aggregator = tokio::spawn(tokio::task::unconstrained(aggregator.run()));
                    let (shutdown, shutting_down) = tokio::sync::oneshot::channel();
                    let server = tonic::transport::Server::builder()
                        .add_service(instrument_server)
                        .serve_with_incoming_shutdown(incoming, async {
                            let _ = shutting_down.await;
                        });
                    tokio::pin!(server);
                    let result = tokio::select! {
                        biased;
                        result = &mut aggregator => {
                            return Err(format!("Console aggregator exited: {result:?}"));
                        },
                        result = &mut server => {
                                Err(format!("Console server exited: {result:?}"))
                            },
                        _ = stopped => Ok(()),
                    };
                    aggregator.abort();
                    let cleanup = match aggregator.await {
                        Err(error) if error.is_cancelled() => Ok(()),
                        result => Err(format!(
                            "Console aggregator exited during cleanup: {result:?}"
                        )),
                    };
                    result.and(cleanup)?;
                    // Dropping the aggregator closes its update streams. Let
                    // HTTP/2 flush those streams instead of cutting a large
                    // update body off when the stop signal arrives.
                    let _ = shutdown.send(());
                    tokio::time::timeout(Duration::from_secs(5), &mut server)
                        .await
                        .map_err(|_| "Console stream shutdown timed out".to_owned())?
                        .map_err(|error| format!("Console stream shutdown failed: {error}"))
                })
                // Dropping this private runtime cancels and joins its server
                // and aggregator tasks before the owning thread returns.
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            stop: Some(stop),
            thread: Some(thread),
        })
    }

    fn finish(&mut self) -> Result<(), String> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| "Console server thread panicked".to_owned())??;
        }
        Ok(())
    }
}

#[cfg(feature = "tokio-trace")]
impl Drop for ConsoleGuard {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            eprintln!("Console cleanup failed: {error}");
        }
    }
}

#[cfg(feature = "profiling")]
struct Observability {
    recorder: Option<ProfileSpanRecorder>,
    #[cfg(feature = "tokio-trace")]
    console: ConsoleGuard,
}

#[cfg(feature = "profiling")]
fn init_observability() -> Result<Observability, String> {
    use tracing_subscriber::Layer;
    use tracing_subscriber::filter::FilterFn;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let recorder = std::env::var_os("TX_POOL_PROFILE_TRACE_PATH")
        .map(|path| {
            let output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|error| {
                    format!("cannot create profile span counters {path:?}: {error}")
                })?;
            Ok::<_, String>(ProfileSpanRecorder::new(output))
        })
        .transpose()?;
    #[cfg(not(feature = "tokio-trace"))]
    if recorder.is_none() {
        return Ok(Observability { recorder });
    }
    let layer = recorder.as_ref().map(|recorder| {
        recorder.layer().with_filter(FilterFn::new(|metadata| {
            metadata.target() == "ckb_tx_pool_profile"
        }))
    });
    let subscriber = tracing_subscriber::registry().with(layer);
    #[cfg(feature = "tokio-trace")]
    let (subscriber, server, listener) = {
        let address = match std::env::var("TOKIO_CONSOLE_BIND") {
            Ok(value) => value,
            Err(std::env::VarError::NotPresent) => "127.0.0.1:6669".into(),
            Err(error) => return Err(error.to_string()),
        }
        .parse::<std::net::SocketAddr>()
        .map_err(|error| format!("invalid TOKIO_CONSOLE_BIND: {error}"))?;
        // Own the socket before publishing the subscriber or starting work.
        // An occupied bind fails synchronously; port zero gets a fresh socket.
        let listener = std::net::TcpListener::bind(address)
            .map_err(|error| format!("cannot bind Tokio Console at {address}: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let (layer, server) = console_subscriber::ConsoleLayer::builder()
            .event_buffer_capacity(262_144)
            .retention(Duration::from_secs(30))
            .publish_interval(Duration::from_millis(50))
            .build();
        let layer = layer.with_filter(FilterFn::new(|metadata| {
            if metadata.is_event() {
                metadata.target().starts_with("runtime") || metadata.target().starts_with("tokio")
            } else {
                metadata.name().starts_with("runtime.") || metadata.target().starts_with("tokio")
            }
        }));
        eprintln!(
            "TOKIO_CONSOLE_BOUND {}",
            listener.local_addr().map_err(|error| error.to_string())?
        );
        (subscriber.with(layer), server, listener)
    };
    subscriber
        .try_init()
        .map_err(|error| format!("cannot install observability subscriber: {error}"))?;
    Ok(Observability {
        recorder,
        #[cfg(feature = "tokio-trace")]
        console: ConsoleGuard::start(server, listener)?,
    })
}

#[repr(align(128))]
struct CompletionCounter(AtomicUsize);

struct Completion {
    accepted_shards: [CompletionCounter; COMPLETION_COUNTER_SHARDS],
    duplicate_callbacks: AtomicUsize,
    unexpected_callbacks: AtomicUsize,
    early_target_callbacks: AtomicUsize,
    changed: Notify,
    callbacks_in_flight: AtomicUsize,
    indexes: HashMap<Byte32, usize>,
    timestamps_ns: Box<[AtomicU64]>,
    target_begin: usize,
    target_started: OnceLock<Instant>,
    track_in_flight: bool,
}

impl Completion {
    fn new(
        transactions: &[TransactionView],
        target_begin: usize,
        track_in_flight: bool,
    ) -> BenchResult<Self> {
        require(
            target_begin <= transactions.len(),
            "callback target boundary exceeds corpus",
        )?;
        let mut indexes = HashMap::new();
        indexes.try_reserve(transactions.len())?;
        for (index, transaction) in transactions.iter().enumerate() {
            require(
                indexes.insert(transaction.hash(), index).is_none(),
                "callback corpus contains a duplicate hash",
            )?;
        }
        let timestamps_ns = std::iter::repeat_with(|| AtomicU64::new(0))
            .take(transactions.len())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Ok(Self {
            accepted_shards: std::array::from_fn(|_| CompletionCounter(AtomicUsize::new(0))),
            duplicate_callbacks: AtomicUsize::new(0),
            unexpected_callbacks: AtomicUsize::new(0),
            early_target_callbacks: AtomicUsize::new(0),
            changed: Notify::new(),
            callbacks_in_flight: AtomicUsize::new(0),
            indexes,
            timestamps_ns,
            target_begin,
            target_started: OnceLock::new(),
            track_in_flight,
        })
    }

    fn begin_callback(&self) {
        if self.track_in_flight {
            self.callbacks_in_flight.fetch_add(1, Ordering::AcqRel);
            self.changed.notify_one();
        }
    }

    fn finish_callback(&self, hash: Byte32) {
        self.record(hash);
        if self.track_in_flight {
            self.callbacks_in_flight.fetch_sub(1, Ordering::Release);
            self.changed.notify_one();
        }
    }

    fn record(&self, hash: Byte32) {
        let Some(&index) = self.indexes.get(&hash) else {
            self.unexpected_callbacks.fetch_add(1, Ordering::AcqRel);
            self.changed.notify_one();
            return;
        };
        let timestamp = if index < self.target_begin {
            1
        } else if let Some(started) = self.target_started.get() {
            started.elapsed().as_nanos().min(u128::from(u64::MAX - 1)) as u64 + 1
        } else {
            self.early_target_callbacks.fetch_add(1, Ordering::AcqRel);
            1
        };
        if self.timestamps_ns[index]
            .compare_exchange(0, timestamp, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            self.duplicate_callbacks.fetch_add(1, Ordering::AcqRel);
            self.changed.notify_one();
            return;
        }
        self.accepted_shards[index % COMPLETION_COUNTER_SHARDS]
            .0
            .fetch_add(1, Ordering::Release);
        self.changed.notify_one();
    }

    fn accepted_count(&self) -> usize {
        self.accepted_shards
            .iter()
            .map(|counter| counter.0.load(Ordering::Acquire))
            .sum()
    }

    fn begin_target(&self, started: Instant) {
        self.target_started
            .set(started)
            .expect("benchmark has one target measurement window");
    }

    fn validate(&self, expected: usize, allow_duplicates: bool) -> BenchResult<()> {
        let observed = self.accepted_count();
        let duplicates = self.duplicate_callbacks.load(Ordering::Acquire);
        let unexpected = self.unexpected_callbacks.load(Ordering::Acquire);
        let early_target = self.early_target_callbacks.load(Ordering::Acquire);
        let callbacks_in_flight = self.callbacks_in_flight.load(Ordering::Acquire);
        require(
            observed == expected
                && (allow_duplicates || duplicates == 0)
                && unexpected == 0
                && early_target == 0
                && callbacks_in_flight == 0,
            format!(
                "callback terminal differs: observed={observed} expected={expected} indexed={} duplicates={duplicates} unexpected={unexpected} early_target={early_target} in_flight={callbacks_in_flight}",
                self.indexes.len(),
            ),
        )
    }

    async fn wait_for_callback_in_flight(&self) -> BenchResult<usize> {
        require(
            self.track_in_flight,
            "callback overlap observation is disabled",
        )?;
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let changed = self.changed.notified();
                let count = self.callbacks_in_flight.load(Ordering::Acquire);
                if count != 0 {
                    break count;
                }
                changed.await;
            }
        })
        .await
        .map_err(|_| bench_error("target callback did not overlap reorg"))
    }

    fn end_target(&self) -> u64 {
        let mut samples = self.timestamps_ns[self.target_begin..]
            .iter()
            .map(|sample| sample.load(Ordering::Acquire))
            .collect::<Vec<_>>();
        for sample in &mut samples {
            *sample -= 1;
        }
        samples.sort_unstable();
        let index = samples
            .len()
            .saturating_mul(99)
            .div_ceil(100)
            .saturating_sub(1);
        samples[index]
    }

    async fn wait_for(&self, target: usize) -> BenchResult<()> {
        tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let changed = self.changed.notified();
                if self.accepted_count() >= target {
                    break;
                }
                changed.await;
            }
        })
        .await
        .map_err(|_| {
            bench_error(format!(
                "timed out after accepting {}/{} transactions",
                self.accepted_count(),
                target
            ))
        })?;
        Ok(())
    }
}

#[derive(Default)]
struct RelayCompletion {
    ok: Mutex<RelayOkSet>,
    duplicate_ok: AtomicUsize,
    rejects: Mutex<RelayRejectSet>,
    duplicate_reject: AtomicUsize,
    unknown_parents: Mutex<BTreeMap<(PeerIndex, Vec<Byte32>), usize>>,
    generation_resets: AtomicUsize,
    changed: Notify,
}

struct RelayObservation {
    ok: usize,
    duplicate_ok: usize,
    rejects: usize,
    unknown_parents: usize,
    unknown_parent_observations: Vec<serde_json::Value>,
    generation_resets: usize,
}

impl RelayCompletion {
    fn record_unknown_parents(&self, peer: PeerIndex, parents: HashSet<Byte32>) {
        let mut parents = parents.into_iter().collect::<Vec<_>>();
        parents.sort_unstable();
        *lock(&self.unknown_parents)
            .entry((peer, parents))
            .or_default() += 1;
    }

    fn record(&self, result: TxVerificationResult) {
        match result {
            TxVerificationResult::Ok {
                original_peer,
                tx_hash,
            } => {
                if !lock(&self.ok).insert((tx_hash, original_peer)) {
                    self.duplicate_ok.fetch_add(1, Ordering::AcqRel);
                }
            }
            TxVerificationResult::Reject { tx_hash } => {
                if !lock(&self.rejects).insert(tx_hash) {
                    self.duplicate_reject.fetch_add(1, Ordering::AcqRel);
                }
            }
            TxVerificationResult::UnknownParents { peer, parents } => {
                self.record_unknown_parents(peer, parents);
            }
            #[cfg(not(feature = "cross-version-legacy-bench-adapter"))]
            TxVerificationResult::GenerationReset => {
                self.generation_resets.fetch_add(1, Ordering::AcqRel);
            }
        }
        self.changed.notify_one();
    }

    fn terminal_counts(&self) -> (usize, usize) {
        (lock(&self.ok).len(), lock(&self.rejects).len())
    }

    fn observation(&self) -> RelayObservation {
        let unknown = lock(&self.unknown_parents);
        let unknown_parent_observations = unknown
            .iter()
            .map(|((peer, parents), count)| {
                serde_json::json!({
                    "peer": peer.value(),
                    "parents": parents
                        .iter()
                        .map(|parent| hex_bytes(parent.as_slice()))
                        .collect::<Vec<_>>(),
                    "count": count,
                })
            })
            .collect();
        RelayObservation {
            ok: lock(&self.ok).len(),
            duplicate_ok: self.duplicate_ok.load(Ordering::Acquire),
            rejects: lock(&self.rejects).len(),
            unknown_parents: unknown.values().sum(),
            unknown_parent_observations,
            generation_resets: self.generation_resets.load(Ordering::Acquire),
        }
    }

    fn reserve(&self, ok: usize, rejects: usize) -> BenchResult<()> {
        lock(&self.ok).try_reserve(ok)?;
        lock(&self.rejects).try_reserve(rejects)?;
        Ok(())
    }

    async fn wait_for_terminals(
        &self,
        expected_ok: usize,
        expected_rejects: usize,
    ) -> BenchResult<()> {
        tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let changed = self.changed.notified();
                let (ok, rejects) = self.terminal_counts();
                if ok >= expected_ok && rejects >= expected_rejects {
                    break;
                }
                changed.await;
            }
        })
        .await
        .map_err(|_| {
            let (ok, rejects) = self.terminal_counts();
            bench_error(format!(
                "timed out after observing {}/{} relay Ok and {}/{} Reject results",
                ok, expected_ok, rejects, expected_rejects
            ))
        })?;
        Ok(())
    }

    fn validate_stress(
        &self,
        completion: &Completion,
        expected: &RelayOkSet,
        allowed: &HashMap<PeerIndex, HashSet<Byte32>>,
    ) -> BenchResult<()> {
        let accepted = expected
            .iter()
            .filter(|(hash, _)| {
                completion.indexes.get(hash).is_some_and(|&index| {
                    completion.timestamps_ns[index].load(Ordering::Acquire) != 0
                })
            })
            .cloned()
            .collect::<RelayOkSet>();
        let rejects = lock(&self.rejects).clone();
        if self.generation_resets.load(Ordering::Acquire) == 0 {
            return self.validate(&accepted, &rejects, Some(allowed));
        }
        // Reset does not replay historical Ok results. Check every retained
        // observation's ownership; the stress report explicitly marks history
        // as incomplete and never emits a throughput result.
        require(
            lock(&self.ok).is_subset(&accepted),
            "stress relay Ok has wrong hash or original peer",
        )?;
        require(
            lock(&self.unknown_parents).keys().all(|(peer, parents)| {
                !parents.is_empty()
                    && parents
                        .iter()
                        .all(|parent| allowed.get(peer).is_some_and(|set| set.contains(parent)))
            }),
            "stress relay requested an unexpected parent",
        )
    }

    fn validate(
        &self,
        expected_ok: &RelayOkSet,
        expected_rejects: &RelayRejectSet,
        allowed_unknown_parents: Option<&HashMap<PeerIndex, HashSet<Byte32>>>,
    ) -> BenchResult<()> {
        let ok = lock(&self.ok);
        require(
            *ok == *expected_ok,
            format!(
                "relay Ok set differs: observed={}, expected={}",
                ok.len(),
                expected_ok.len()
            ),
        )?;
        let rejects = lock(&self.rejects);
        require(
            *rejects == *expected_rejects,
            format!(
                "relay Reject set differs: observed={}, expected={}",
                rejects.len(),
                expected_rejects.len()
            ),
        )?;
        let duplicate_ok = self.duplicate_ok.load(Ordering::Acquire);
        let duplicate_reject = self.duplicate_reject.load(Ordering::Acquire);
        let unknown_parent_observations = lock(&self.unknown_parents);
        let unknown_parents: usize = unknown_parent_observations.values().sum();
        let invalid_unknown_parent = match allowed_unknown_parents {
            Some(allowed) => unknown_parent_observations.keys().any(|(peer, parents)| {
                parents.is_empty()
                    || parents
                        .iter()
                        .any(|parent| !allowed.get(peer).is_some_and(|set| set.contains(parent)))
            }),
            None => !unknown_parent_observations.is_empty(),
        };
        let generation_resets = self.generation_resets.load(Ordering::Acquire);
        require(
            duplicate_ok == 0
                && duplicate_reject == 0
                && generation_resets == 0
                && !invalid_unknown_parent,
            format!(
                "relay terminal stream contains duplicate_ok={duplicate_ok} duplicate_reject={duplicate_reject} unknown_parents={unknown_parents} generation_resets={generation_resets}"
            ),
        )
    }
}

/// Emit the actual partial terminal state on every early return, including
/// submission and callback timeouts which otherwise suppress relay diagnostics.
struct TerminalDiagnosticsGuard {
    completion: Arc<Completion>,
    relay: Arc<RelayCompletion>,
    armed: bool,
}

fn terminal_diagnostics(completion: &Completion, relay: &RelayCompletion) -> serde_json::Value {
    let rejects = lock(&relay.rejects);
    let accepted = completion
        .indexes
        .iter()
        .filter_map(|(hash, &index)| {
            (completion.timestamps_ns[index].load(Ordering::Acquire) != 0).then_some(hash.clone())
        })
        .collect::<HashSet<_>>();
    let mut unresolved = completion
        .indexes
        .keys()
        .filter(|hash| !accepted.contains(*hash) && !rejects.contains(*hash))
        .map(|hash| hex_bytes(hash.as_slice()))
        .collect::<Vec<_>>();
    unresolved.sort_unstable();
    let mut rejected_hashes = rejects
        .iter()
        .map(|hash| hex_bytes(hash.as_slice()))
        .collect::<Vec<_>>();
    rejected_hashes.sort_unstable();
    let overlapping = accepted.intersection(&rejects).count();
    let unexpected_rejects = rejects
        .iter()
        .filter(|hash| !completion.indexes.contains_key(*hash))
        .count();
    let rejected = rejects.len();
    drop(rejects);
    let observation = relay.observation();
    let mut accepted_hashes = accepted
        .iter()
        .map(|hash| hex_bytes(hash.as_slice()))
        .collect::<Vec<_>>();
    accepted_hashes.sort_unstable();
    let mut relay_ok = lock(&relay.ok)
        .iter()
        .map(|(hash, peer)| (hex_bytes(hash.as_slice()), peer.map(|peer| peer.value())))
        .collect::<Vec<_>>();
    relay_ok.sort_unstable();
    serde_json::json!({
        "planned": completion.indexes.len(), "accepted": accepted.len(),
        "unresolved_scope": "planned corpus without a terminal; may include transactions not submitted",
        "accepted_hashes": accepted_hashes, "relay_ok_observations": relay_ok,
        "rejected": rejected, "rejected_hashes": rejected_hashes,
        "unresolved": unresolved.len(), "unresolved_hashes": unresolved,
        "accepted_rejected_overlap": overlapping, "unexpected_rejects": unexpected_rejects,
        "callback_duplicates": completion.duplicate_callbacks.load(Ordering::Acquire),
        "unexpected_callbacks": completion.unexpected_callbacks.load(Ordering::Acquire),
        "relay_ok": observation.ok, "relay_duplicate_ok": observation.duplicate_ok,
        "relay_duplicate_reject": relay.duplicate_reject.load(Ordering::Acquire),
        "relay_generation_resets": observation.generation_resets,
        "relay_unknown_parent_observations": observation.unknown_parent_observations,
    })
}

impl Drop for TerminalDiagnosticsGuard {
    fn drop(&mut self) {
        if self.armed {
            eprintln!(
                "BENCH_FAILURE_TERMINALS {}",
                terminal_diagnostics(&self.completion, &self.relay)
            );
        }
    }
}

async fn wait_for_stress_settlement(
    completion: &Completion,
    relay: &RelayCompletion,
    deadline: tokio::time::Instant,
) -> bool {
    tokio::time::timeout_at(deadline, async {
        loop {
            let callback_changed = completion.changed.notified();
            let relay_changed = relay.changed.notified();
            let settled = {
                let rejects = lock(&relay.rejects);
                completion.indexes.iter().all(|(hash, &index)| {
                    completion.timestamps_ns[index].load(Ordering::Acquire) != 0
                        || rejects.contains(hash)
                })
            };
            let (ok, _) = relay.terminal_counts();
            if settled
                && (ok >= completion.accepted_count()
                    || relay.generation_resets.load(Ordering::Acquire) != 0)
            {
                break;
            }
            tokio::select! { _ = callback_changed => {}, _ = relay_changed => {} }
        }
    })
    .await
    .is_ok()
}

#[cfg(feature = "cross-version-legacy-bench-adapter")]
type BenchmarkRelayReceiver = ckb_channel::Receiver<TxVerificationResult>;
#[cfg(not(feature = "cross-version-legacy-bench-adapter"))]
type BenchmarkRelayReceiver = TxVerificationResultReceiver;

#[cfg(feature = "cross-version-legacy-bench-adapter")]
fn try_recv_relay(receiver: &BenchmarkRelayReceiver) -> Option<TxVerificationResult> {
    receiver.try_recv().ok()
}

#[cfg(not(feature = "cross-version-legacy-bench-adapter"))]
fn try_recv_relay(receiver: &BenchmarkRelayReceiver) -> Option<TxVerificationResult> {
    receiver.try_recv()
}

struct RelayDrainGuard {
    stop: Arc<AtomicBool>,
    completion: Arc<RelayCompletion>,
    handle: std::thread::JoinHandle<()>,
}

impl RelayDrainGuard {
    fn start(receiver: BenchmarkRelayReceiver) -> BenchResult<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let completion = Arc::new(RelayCompletion::default());
        let thread_stop = Arc::clone(&stop);
        let thread_completion = Arc::clone(&completion);
        #[cfg(not(feature = "cross-version-legacy-bench-adapter"))]
        let timer = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()?;
        let handle = std::thread::Builder::new()
            .name("txpool-bench-relay-drain".to_owned())
            .spawn(move || {
                loop {
                    let mut drained = false;
                    while let Some(result) = try_recv_relay(&receiver) {
                        thread_completion.record(result);
                        drained = true;
                    }
                    if thread_stop.load(Ordering::Acquire) && !drained {
                        break;
                    }
                    if !drained {
                        #[cfg(not(feature = "cross-version-legacy-bench-adapter"))]
                        timer.block_on(async {
                            // A sparse ordinary outcome has no producer wake; keep
                            // a bounded fallback, also bounding stop/join latency.
                            let _ = tokio::time::timeout(
                                Duration::from_millis(1),
                                receiver.wait_for_drain(),
                            )
                            .await;
                        });
                        #[cfg(feature = "cross-version-legacy-bench-adapter")]
                        match receiver.recv_timeout(Duration::from_millis(1)) {
                            Ok(result) => thread_completion.record(result),
                            Err(ckb_channel::RecvTimeoutError::Timeout) => {}
                            Err(ckb_channel::RecvTimeoutError::Disconnected) => break,
                        }
                    }
                }
            })?;
        Ok(Self {
            stop,
            completion,
            handle,
        })
    }

    fn completion(&self) -> Arc<RelayCompletion> {
        Arc::clone(&self.completion)
    }

    fn stop(self) -> BenchResult<()> {
        self.stop.store(true, Ordering::Release);
        self.handle
            .join()
            .map_err(|_| bench_error("relay drain thread panicked"))?;
        Ok(())
    }
}

#[cfg(not(feature = "cross-version-legacy-bench-adapter"))]
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

#[cfg(feature = "cross-version-legacy-bench-adapter")]
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

fn genesis_consensus(transactions: Vec<TransactionView>) -> Consensus {
    let dao = genesis_dao_data(transactions.iter().collect()).expect("valid genesis DAO");
    let genesis = transactions
        .into_iter()
        .fold(
            BlockBuilder::default()
                .timestamp(1_557_310_743u64)
                .compact_target(difficulty_to_compact(U256::from(1_000u64)))
                .dao(dao),
            |builder, transaction| builder.transaction(transaction),
        )
        .build();
    ConsensusBuilder::default()
        .genesis_block(genesis)
        .cellbase_maturity(EpochNumberWithFraction::new(0, 0, 1))
        .build()
}

fn issue_transaction(lock: Script, capacity: Capacity, outputs: usize) -> TransactionView {
    let output = CellOutput::new_builder()
        .capacity(capacity)
        .lock(lock)
        .build();
    TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .outputs(std::iter::repeat_n(output, outputs))
        .outputs_data(std::iter::repeat_n(Bytes::default().pack(), outputs))
        .build()
}

fn test_consensus(issue_outputs: usize) -> (Consensus, TransactionView) {
    test_consensus_with_capacity(
        issue_outputs,
        Capacity::bytes(ISSUE_CAPACITY_BYTES).expect("valid issue capacity"),
    )
}

fn test_consensus_with_capacity(
    issue_outputs: usize,
    capacity: Capacity,
) -> (Consensus, TransactionView) {
    let (always_success_cell, always_success_data, always_success_script) = always_success_cell();
    let always_success_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .output_data(always_success_data.clone())
        .witness(always_success_script.clone().into_witness())
        .build();
    let issue_tx = issue_transaction(always_success_script.clone(), capacity, issue_outputs);
    (
        genesis_consensus(vec![always_success_tx, issue_tx.clone()]),
        issue_tx,
    )
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

fn snapshot_with_proposed(
    base: &Snapshot,
    store: &MockStore,
    proposals: impl IntoIterator<Item = ckb_types::packed::ProposalShortId>,
) -> Arc<Snapshot> {
    let proposals = proposals.into_iter().collect::<HashSet<_>>();
    Arc::new(Snapshot::new(
        base.tip_header().clone(),
        base.total_difficulty().clone(),
        base.epoch_ext().clone(),
        store.store().get_snapshot(),
        ProposalView::new(HashSet::new(), proposals),
        base.cloned_consensus(),
    ))
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

fn build_success_tx(
    inputs: impl IntoIterator<Item = OutPoint>,
    output_bytes: impl IntoIterator<Item = usize>,
) -> TransactionView {
    let lock = always_success_script();
    let outputs = output_bytes
        .into_iter()
        .map(|bytes| {
            CellOutput::new_builder()
                .capacity(Capacity::bytes(bytes).expect("valid output capacity"))
                .lock(lock.clone())
                .build()
        })
        .collect::<Vec<_>>();
    let output_count = outputs.len();
    TransactionBuilder::default()
        .cell_dep(always_success_dep())
        .inputs(inputs.into_iter().map(|input| CellInput::new(input, 0)))
        .outputs(outputs)
        .outputs_data(std::iter::repeat_n(Bytes::default().pack(), output_count))
        .build()
}

fn build_tx_with_output_bytes(input: OutPoint, output_bytes: usize) -> TransactionView {
    build_success_tx([input], [output_bytes])
}

fn build_tx(input: OutPoint) -> TransactionView {
    build_tx_with_output_bytes(input, 100)
}

fn build_multi_input_tx(inputs: impl IntoIterator<Item = OutPoint>) -> TransactionView {
    build_success_tx(inputs, [100])
}

fn build_fanout_parent(input: OutPoint, output_count: usize) -> TransactionView {
    build_success_tx(
        [input],
        std::iter::repeat_n(FANOUT_OUTPUT_CAPACITY_BYTES, output_count),
    )
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
    let issue_tx = issue_transaction(
        secp_script(),
        Capacity::shannons(SECP_ISSUE_CAPACITY),
        issue_outputs,
    );
    (
        genesis_consensus(vec![system_tx, issue_tx.clone()]),
        issue_tx,
        cell_deps,
    )
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

fn build_chain(mut input: OutPoint, count: usize) -> Vec<TransactionView> {
    (0..count)
        .map(|_| {
            let transaction = build_tx(input.clone());
            input = OutPoint::new(transaction.hash(), 0);
            transaction
        })
        .collect()
}

fn build_workload(
    scenario: &str,
    transaction_count: usize,
) -> BenchResult<(Consensus, Vec<TransactionView>)> {
    if let Some(depth_spec) = scenario.strip_prefix("dependent_forest_") {
        let (depth, reverse) = match depth_spec.strip_suffix("_reverse") {
            Some(depth) => (depth, true),
            None => (depth_spec, false),
        };
        let depth: usize = depth.parse()?;
        require(depth != 0, "dependency depth must be non-zero")?;
        let chain_count = transaction_count.div_ceil(depth);
        let (consensus, issue_tx) = test_consensus(chain_count);
        let mut transactions = (0..chain_count)
            .flat_map(|chain| {
                build_chain(
                    OutPoint::new(
                        issue_tx.hash(),
                        u32::try_from(chain).expect("bounded index"),
                    ),
                    depth.min(transaction_count - chain * depth),
                )
            })
            .collect::<Vec<_>>();
        if reverse {
            transactions.reverse();
        }
        return Ok((consensus, transactions));
    }
    if let Some(fan_in) = scenario.strip_prefix("always_success_fanin_") {
        let fan_in: usize = fan_in.parse()?;
        require(fan_in != 0, "fan-in must be non-zero")?;
        let issue_outputs = transaction_count
            .checked_mul(fan_in)
            .ok_or_else(|| bench_error("fan-in workload size overflow"))?;
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
        "rbf_pairs" => {
            require(
                transaction_count.is_multiple_of(2),
                "RBF workload requires equal victim and replacement halves",
            )?;
            let pair_count = transaction_count / 2;
            let (consensus, issue_tx) = test_consensus(pair_count);
            let (mut victims, replacements): (Vec<_>, Vec<_>) = (0..pair_count)
                .map(|index| {
                    let input = OutPoint::new(
                        issue_tx.hash(),
                        u32::try_from(index).expect("transaction population fits output indexes"),
                    );
                    (
                        build_tx_with_output_bytes(input.clone(), 101),
                        build_tx_with_output_bytes(input, 100),
                    )
                })
                .unzip();
            victims.extend(replacements);
            Ok((consensus, victims))
        }
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
        "dependent" | "dependent_reverse" => {
            let (consensus, issue_tx) = test_consensus(1);
            let mut transactions =
                build_chain(OutPoint::new(issue_tx.hash(), 0), transaction_count);
            if scenario.ends_with("_reverse") {
                transactions.reverse();
            }
            Ok((consensus, transactions))
        }
        "fanout_ready_64_reverse" => {
            require(
                transaction_count != 0 && transaction_count.is_multiple_of(FANOUT_COHORT_SIZE),
                "fanout cohorts require a positive multiple of 65 transactions",
            )?;
            let cohort_count = transaction_count / FANOUT_COHORT_SIZE;
            let (consensus, issue_tx) =
                test_consensus_with_capacity(cohort_count, Capacity::bytes(ISSUE_CAPACITY_BYTES)?);
            let mut transactions = Vec::with_capacity(transaction_count);
            for cohort in 0..cohort_count {
                let parent = build_fanout_parent(
                    OutPoint::new(issue_tx.hash(), u32::try_from(cohort)?),
                    FANOUT_COHORT_SIZE - 1,
                );
                require(
                    parent.data().serialized_size_in_block() as u64 <= TRANSACTION_SIZE_LIMIT,
                    "fanout cohort parent exceeds transaction size limit",
                )?;
                transactions.extend(
                    (0..FANOUT_COHORT_SIZE - 1)
                        .rev()
                        .map(|index| build_tx(OutPoint::new(parent.hash(), index as u32))),
                );
                transactions.push(parent);
            }
            Ok((consensus, transactions))
        }
        "fanout" | "fanout_reverse" => {
            require(
                transaction_count >= 2,
                "fanout requires a parent and a child",
            )?;
            let child_count = transaction_count - 1;
            // Derive the encoded size from this fixture's actual shape before
            // allocating the full output vector. Each output and empty data item
            // adds a fixed payload and offset to the two Molecule vectors.
            let parent_size = |count| {
                build_fanout_parent(OutPoint::null(), count)
                    .data()
                    .serialized_size_in_block()
            };
            let base_size = parent_size(0);
            let output_size = parent_size(1) - base_size;
            let size = output_size
                .checked_mul(child_count)
                .and_then(|outputs| base_size.checked_add(outputs))
                .ok_or_else(|| bench_error("fanout parent size overflow"))?;
            require(
                u64::try_from(size)? <= TRANSACTION_SIZE_LIMIT,
                format!("fanout parent size {size} exceeds limit {TRANSACTION_SIZE_LIMIT}"),
            )?;
            let capacity_bytes = child_count
                .checked_mul(FANOUT_OUTPUT_CAPACITY_BYTES)
                .ok_or_else(|| bench_error("fanout capacity overflow"))?
                .max(ISSUE_CAPACITY_BYTES);
            let (consensus, issue_tx) =
                test_consensus_with_capacity(1, Capacity::bytes(capacity_bytes)?);
            let parent = build_fanout_parent(OutPoint::new(issue_tx.hash(), 0), child_count);
            require(
                parent.data().serialized_size_in_block() == size,
                "fanout parent size differs from its preflight",
            )?;
            let mut children = (0..child_count)
                .map(|index| build_tx(OutPoint::new(parent.hash(), index as u32)))
                .collect::<Vec<_>>();
            if scenario == "fanout_reverse" {
                children.reverse();
                children.push(parent);
            } else {
                children.insert(0, parent);
            }
            Ok((consensus, children))
        }
        _ => Err(bench_error(format!("unknown scenario: {scenario}"))),
    }
}

fn peer_ranges(len: usize, peers: usize) -> Vec<(usize, usize)> {
    let chunk = len.div_ceil(peers.max(1)).max(1);
    (0..len)
        .step_by(chunk)
        .map(|start| (start, (start + chunk).min(len)))
        .collect()
}

mod fanout_readiness;
mod measurement_clock;
#[cfg(not(feature = "cross-version-legacy-bench-adapter"))]
mod relay_batches;
mod resource_phases;

#[cfg(not(feature = "cross-version-legacy-bench-adapter"))]
async fn submit_requests(
    controller: &TxPoolController,
    transactions: Arc<Vec<TransactionView>>,
    cycles: Arc<Vec<u64>>,
    peers: usize,
) -> BenchResult<()> {
    let ranges = peer_ranges(transactions.len(), peers);
    let mut responses = Vec::with_capacity(ranges.len());
    for (peer, (mut start, end)) in ranges.into_iter().enumerate() {
        while start < end {
            let count = relay_batches::batch_len(
                transactions[start..end]
                    .iter()
                    .map(|transaction| transaction.data().total_size()),
            )
            .map_err(bench_error)?;
            let batch = (start..start + count)
                .map(|index| (transactions[index].clone(), cycles[index]))
                .collect();
            let response = controller
                .submit_remote_txs(batch, (peer + 1).into())
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            responses.push(async move {
                let outcome = response
                    .await
                    .map_err(|error| std::io::Error::other(error.to_string()))?;
                let (offered, completed, error) = outcome.into_parts();
                if offered != count || completed != offered || error.is_some() {
                    return Err(std::io::Error::other(format!(
                        "remote batch outcome mismatch: offered={offered}, completed={completed}, error={error:?}"
                    )));
                }
                Ok(())
            });
            start += count;
        }
    }
    for response in futures_util::future::join_all(responses).await {
        response?;
    }
    Ok(())
}

#[cfg(feature = "cross-version-legacy-bench-adapter")]
async fn submit_requests(
    controller: &TxPoolController,
    transactions: Arc<Vec<TransactionView>>,
    cycles: Arc<Vec<u64>>,
    peers: usize,
) -> BenchResult<()> {
    let ranges = peer_ranges(transactions.len(), peers);
    let barrier = Arc::new(Barrier::new(ranges.len() + 1));
    // Dropping the set on a failed response or deadline aborts all remaining
    // peer submissions. Detached JoinHandles would keep submitting afterwards.
    let mut submissions = tokio::task::JoinSet::new();
    for (peer, (start, end)) in ranges.into_iter().enumerate() {
        let controller = controller.clone();
        let transactions = Arc::clone(&transactions);
        let cycles = Arc::clone(&cycles);
        let barrier = Arc::clone(&barrier);
        submissions.spawn(async move {
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
        });
    }
    barrier.wait().await;
    while let Some(submission) = submissions.join_next().await {
        submission??;
    }
    Ok(())
}

async fn submit_only(
    controller: &TxPoolController,
    transactions: Arc<Vec<TransactionView>>,
    cycles: Arc<Vec<u64>>,
    peers: usize,
) -> BenchResult<()> {
    // Leave time for terminal diagnostics and cleanup before the external
    // process deadline. Admission replies cannot bypass every internal timer.
    tokio::time::timeout(
        Duration::from_secs(90),
        submit_requests(controller, transactions, cycles, peers),
    )
    .await
    .map_err(|_| bench_error("remote submission exceeded 90 seconds"))?
}

async fn submit_batch(
    controller: &TxPoolController,
    completion: &Completion,
    transactions: Arc<Vec<TransactionView>>,
    cycles: Arc<Vec<u64>>,
    peers: usize,
    expected_total: usize,
) -> BenchResult<()> {
    submit_only(controller, transactions, cycles, peers).await?;
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
) -> BenchResult<()> {
    require(
        depth != 0 && transactions.len().is_multiple_of(depth),
        "dependency-forest batch must contain complete non-empty chains",
    )?;
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

#[derive(Clone, Copy)]
enum SubmissionOrder {
    Concurrent,
    Forest(usize),
    ParentFirst,
    ReverseFanoutCohorts,
}

async fn submit_workload(
    controller: &TxPoolController,
    completion: &Completion,
    transactions: Arc<Vec<TransactionView>>,
    cycles: Arc<Vec<u64>>,
    order: SubmissionOrder,
    peers: usize,
    accepted_before: usize,
) -> BenchResult<fanout_readiness::Readiness> {
    let mut readiness = fanout_readiness::Readiness::default();
    if matches!(order, SubmissionOrder::ReverseFanoutCohorts) {
        require(
            transactions.len().is_multiple_of(FANOUT_COHORT_SIZE),
            "incomplete fanout cohort",
        )?;
        for (cohort, (transactions, cycles)) in transactions
            .chunks_exact(FANOUT_COHORT_SIZE)
            .zip(cycles.chunks_exact(FANOUT_COHORT_SIZE))
            .enumerate()
        {
            let query = || {
                controller
                    .get_tx_pool_info()
                    .map(|info| info.orphan_size)
                    .map_err(|error| error.to_string())
            };
            readiness.wait(0, query).await.map_err(bench_error)?;
            let parent = FANOUT_COHORT_SIZE - 1;
            submit_only(
                controller,
                Arc::new(transactions[..parent].to_vec()),
                Arc::new(cycles[..parent].to_vec()),
                peers,
            )
            .await?;
            // Only these 64 unique children have been submitted since the
            // observed empty orphan pool. Thus size=64 proves all are stored.
            readiness.wait(parent, query).await.map_err(bench_error)?;
            submit_batch(
                controller,
                completion,
                Arc::new(transactions[parent..].to_vec()),
                Arc::new(cycles[parent..].to_vec()),
                peers,
                accepted_before + (cohort + 1) * FANOUT_COHORT_SIZE,
            )
            .await?;
        }
    } else if let SubmissionOrder::Forest(depth) = order {
        submit_dependency_forest(
            controller,
            completion,
            &transactions,
            &cycles,
            depth,
            peers,
            accepted_before,
        )
        .await?;
    } else if matches!(order, SubmissionOrder::ParentFirst) && !transactions.is_empty() {
        // A forward fanout promises no missing-parent notices. Its ordering in
        // the vector does not order verification across remote batches/workers.
        // Wait for the parent's accepted callback, then admit children together.
        submit_batch(
            controller,
            completion,
            Arc::new(transactions[..1].to_vec()),
            Arc::new(cycles[..1].to_vec()),
            peers,
            accepted_before + 1,
        )
        .await?;
        submit_batch(
            controller,
            completion,
            Arc::new(transactions[1..].to_vec()),
            Arc::new(cycles[1..].to_vec()),
            peers,
            accepted_before + transactions.len(),
        )
        .await?;
    } else {
        let expected = accepted_before + transactions.len();
        submit_batch(
            controller,
            completion,
            transactions,
            cycles,
            peers,
            expected,
        )
        .await?;
    }
    Ok(readiness)
}

fn extend_expected_relay_batch(
    expected: &mut RelayOkSet,
    transactions: &[&TransactionView],
    peers: usize,
) {
    for (peer, (start, end)) in peer_ranges(transactions.len(), peers)
        .into_iter()
        .enumerate()
    {
        let peer = Some((peer + 1).into());
        for transaction in &transactions[start..end] {
            expected.insert((transaction.hash(), peer));
        }
    }
}

fn expected_relay_batch(
    transactions: &[TransactionView],
    order: SubmissionOrder,
    peers: usize,
) -> RelayOkSet {
    let mut expected = HashSet::with_capacity(transactions.len());
    if matches!(order, SubmissionOrder::ReverseFanoutCohorts) {
        for cohort in transactions.chunks_exact(FANOUT_COHORT_SIZE) {
            let parent = FANOUT_COHORT_SIZE - 1;
            extend_expected_relay_batch(
                &mut expected,
                &cohort[..parent].iter().collect::<Vec<_>>(),
                peers,
            );
            extend_expected_relay_batch(&mut expected, &[&cohort[parent]], peers);
        }
    } else if let SubmissionOrder::Forest(depth) = order {
        let chain_count = transactions.len() / depth;
        for level in 0..depth {
            let layer = (0..chain_count)
                .map(|chain| &transactions[chain * depth + level])
                .collect::<Vec<_>>();
            extend_expected_relay_batch(&mut expected, &layer, peers);
        }
    } else if matches!(order, SubmissionOrder::ParentFirst) && !transactions.is_empty() {
        extend_expected_relay_batch(&mut expected, &[&transactions[0]], peers);
        let children = transactions[1..].iter().collect::<Vec<_>>();
        extend_expected_relay_batch(&mut expected, &children, peers);
    } else {
        let transactions = transactions.iter().collect::<Vec<_>>();
        extend_expected_relay_batch(&mut expected, &transactions, peers);
    }
    expected
}

/// The reverse fixture may only request actual in-corpus input parents from
/// the peer that supplied that child. Notification multiplicity is not fixed.
fn expected_unknown_parents(
    transactions: &[TransactionView],
    expected: &RelayOkSet,
    corpus_hashes: &HashSet<Byte32>,
) -> HashMap<PeerIndex, HashSet<Byte32>> {
    let peers = expected
        .iter()
        .filter_map(|(hash, peer)| peer.map(|peer| (hash, peer)))
        .collect::<HashMap<_, _>>();
    let mut allowed = HashMap::<PeerIndex, HashSet<Byte32>>::new();
    for transaction in transactions {
        if let Some(peer) = peers.get(&transaction.hash()) {
            for input in transaction.input_pts_iter() {
                let parent = input.tx_hash();
                if corpus_hashes.contains(&parent) {
                    allowed.entry(*peer).or_default().insert(parent);
                }
            }
        }
    }
    allowed
}

fn main() -> BenchResult<()> {
    let mut resource_phases =
        resource_phases::ResourcePhases::from_environment().map_err(bench_error)?;
    resource_phases
        .capture("process_start")
        .map_err(bench_error)?;
    let mut args = std::env::args().skip(1);
    let scenario = args.next().unwrap_or_else(|| "always_success".to_owned());
    let mut number = |default| match args.next() {
        Some(value) => value.parse::<usize>().map_err(bench_error),
        None => Ok(default),
    };
    let target_count = number(1_000)?;
    let warm_count = number(100)?;
    let workers = number(8)?;
    let peers = number(8)?;
    require(
        target_count != 0 && workers != 0 && peers != 0,
        "target, workers and peers must be non-zero",
    )?;
    let callback_delay_us = scenario
        .strip_prefix("always_success_callback_")
        .and_then(|value| value.strip_suffix("us"))
        .map(str::parse::<u64>)
        .transpose()?
        .or_else(|| (scenario == "reorg_in_flight").then_some(500));
    let reorg_in_flight = scenario == "reorg_in_flight";
    let workload_scenario = if callback_delay_us.is_some() || reorg_in_flight {
        "always_success"
    } else {
        scenario.as_str()
    };
    require(
        workload_scenario != "rbf_pairs" || warm_count == target_count,
        "RBF workload requires equal warm and target counts",
    )?;
    require(
        !workload_scenario.ends_with("_reverse")
            || workload_scenario == "fanout_ready_64_reverse"
            || warm_count == 0,
        "reverse dependency workloads require warm=0",
    )?;
    require(
        workload_scenario != "fanout_ready_64_reverse"
            || (target_count.is_multiple_of(FANOUT_COHORT_SIZE)
                && warm_count.is_multiple_of(FANOUT_COHORT_SIZE)),
        "fanout cohort target and warm counts must each be multiples of 65",
    )?;
    // This changes only the observer's expected contract. The separately built
    // diagnostic candidate must actually omit victim effects; normal production
    // with this expectation still fails exact terminal validation.
    let omit_rbf_victim_notices = match std::env::var("TX_POOL_BENCH_COMPARISON_CONTRACT") {
        Ok(value) if value == "rbf-victim-notice-ablation" => true,
        Ok(value) if value == "protocol" => false,
        Err(std::env::VarError::NotPresent) => false,
        _ => return Err(bench_error("unsupported benchmark comparison contract")),
    };
    require(
        !omit_rbf_victim_notices
            || (workload_scenario == "rbf_pairs"
                && !cfg!(feature = "cross-version-legacy-bench-adapter")),
        "victim-notice ablation requires bounded-batch RBF",
    )?;
    let transaction_count = target_count
        .checked_add(warm_count)
        .ok_or_else(|| bench_error("target and warm transaction count overflow"))?;
    // Task creation must be observed, including long-lived pool background
    // tasks. Install the composed subscriber before constructing the runtime.
    #[cfg(feature = "profiling")]
    let mut observability = init_observability().map_err(std::io::Error::other)?;
    let (consensus, transactions) = build_workload(workload_scenario, transaction_count)?;
    resource_phases
        .capture("fixture_ready")
        .map_err(bench_error)?;
    let runtime_threads = std::thread::available_parallelism().map_or(8, |count| count.get());
    let (handle, _handle_stop, runtime) = new_global_runtime(Some(runtime_threads));
    let consensus = Arc::new(consensus);
    let (store, snapshot) = snapshot_with_genesis(Arc::clone(&consensus));
    let (_network_directory, network) = start_network(&consensus, &handle)?;
    #[cfg(feature = "cross-version-legacy-bench-adapter")]
    let (mut builder, controller, relay_receiver) = {
        let (relay_sender, relay_receiver) = ckb_channel::unbounded();
        let (builder, controller) = TxPoolServiceBuilder::new(
            tx_pool_config(workers, workload_scenario == "rbf_pairs"),
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
    let (mut builder, controller, relay_receiver) = {
        let (builder, controller, relay_receiver) = TxPoolServiceBuilder::new(
            tx_pool_config(workers, workload_scenario == "rbf_pairs"),
            Arc::clone(&snapshot),
            None,
            Arc::new(RwLock::new(init_cache())),
            &handle,
            FeeEstimator::new_dummy(),
        )
        .map_err(|error| std::io::Error::other(error.to_string()))?;
        (builder, controller, relay_receiver)
    };
    let relay_guard = RelayDrainGuard::start(relay_receiver)?;
    let relay_completion = relay_guard.completion();
    let completion = Arc::new(Completion::new(&transactions, warm_count, reorg_in_flight)?);
    let mut diagnostics = TerminalDiagnosticsGuard {
        completion: Arc::clone(&completion),
        relay: Arc::clone(&relay_completion),
        armed: true,
    };
    let pending_completion = Arc::clone(&completion);
    builder.register_pending(Box::new(move |entry| {
        pending_completion.begin_callback();
        if let Some(delay) = callback_delay_us {
            std::thread::sleep(Duration::from_micros(delay));
        }
        pending_completion.finish_callback(entry.transaction().hash());
    }));
    let proposed_completion = Arc::clone(&completion);
    builder.register_proposed(Box::new(move |entry| {
        proposed_completion.begin_callback();
        if let Some(delay) = callback_delay_us {
            std::thread::sleep(Duration::from_micros(delay));
        }
        proposed_completion.finish_callback(entry.transaction().hash());
    }));
    builder.start(network);
    controller
        .get_tx_pool_info()
        .map_err(|error| std::io::Error::other(error.to_string()))?;

    resource_phases
        .capture("service_ready")
        .map_err(bench_error)?;
    let sample_cycles = |transaction: &TransactionView| {
        controller
            .test_accept_tx(transaction.clone())
            .map_err(|error| std::io::Error::other(error.to_string()))?
            .map_err(|error| std::io::Error::other(error.to_string()))
            .map(|completed| completed.cycles)
    };
    let (cycles, script_preflight_count) = if workload_scenario == "secp256k1" {
        // Cycle sampling must not prime the measured pool's script cache.
        // test_accept_tx publishes a cache proof in some pool implementations
        // but not others. An isolated canonical verifier gives every side the
        // same declared cycles while leaving each target transaction cold.
        let environment = Arc::new(TxVerifyEnv::new_submit(snapshot.tip_header()));
        let cycles = transactions
            .iter()
            .map(|transaction| {
                let resolved = resolve_transaction(
                    transaction.clone(),
                    &mut HashSet::<OutPoint>::new(),
                    snapshot.as_ref(),
                    snapshot.as_ref(),
                )?;
                TransactionScriptsVerifier::new(
                    Arc::new(resolved),
                    snapshot.as_data_loader(),
                    Arc::clone(&consensus),
                    Arc::clone(&environment),
                )
                .verify(consensus.max_block_cycles())
                .map_err(bench_error)
            })
            .collect::<BenchResult<Vec<_>>>()?;
        let script_preflight_count = cycles.len();
        (cycles, script_preflight_count)
    } else {
        let sample = if workload_scenario.ends_with("_reverse") {
            transactions.last()
        } else {
            transactions.first()
        }
        .ok_or_else(|| std::io::Error::other("benchmark workload must not be empty"))?;
        (vec![sample_cycles(sample)?; transactions.len()], 1)
    };
    let corpus = corpus_observation(&consensus, &transactions, &cycles, script_preflight_count)?;
    resource_phases
        .capture("preflight_complete")
        .map_err(bench_error)?;

    let warm = Arc::new(transactions[..warm_count].to_vec());
    let target = Arc::new(transactions[warm_count..].to_vec());
    let warm_cycles = Arc::new(cycles[..warm_count].to_vec());
    let target_cycles = Arc::new(cycles[warm_count..].to_vec());

    let order = workload_scenario
        .strip_prefix("dependent_forest_")
        .filter(|depth| !depth.ends_with("_reverse"))
        .map(str::parse::<usize>)
        .transpose()?
        .map_or(SubmissionOrder::Concurrent, SubmissionOrder::Forest);
    let order = if workload_scenario == "fanout_ready_64_reverse" {
        SubmissionOrder::ReverseFanoutCohorts
    } else {
        order
    };
    let warm_order = if workload_scenario == "fanout" && warm_count != 0 {
        SubmissionOrder::ParentFirst
    } else {
        order
    };
    let target_order = if workload_scenario == "fanout" && warm_count == 0 {
        SubmissionOrder::ParentFirst
    } else {
        order
    };
    let warm_expected_relay = expected_relay_batch(&warm, warm_order, peers);
    let target_expected_relay = expected_relay_batch(&target, target_order, peers);
    let mut all_expected_relay = warm_expected_relay.clone();
    all_expected_relay.extend(target_expected_relay);
    let warm_expected_rejects = RelayRejectSet::new();
    let all_expected_rejects = if workload_scenario == "rbf_pairs"
        && !cfg!(feature = "cross-version-legacy-bench-adapter")
        && !omit_rbf_victim_notices
    {
        warm.iter().map(TransactionView::hash).collect()
    } else {
        RelayRejectSet::new()
    };
    let corpus_hashes = transactions
        .iter()
        .map(TransactionView::hash)
        .collect::<HashSet<_>>();
    let (allowed_unknown, warm_allowed_unknown) = if workload_scenario.ends_with("_reverse") {
        (
            expected_unknown_parents(&transactions, &all_expected_relay, &corpus_hashes),
            expected_unknown_parents(
                &transactions[..warm_count],
                &warm_expected_relay,
                &corpus_hashes,
            ),
        )
    } else {
        (HashMap::new(), HashMap::new())
    };
    relay_completion.reserve(transactions.len(), all_expected_rejects.len())?;
    if workload_scenario == "fanout_reverse" {
        // Capacity stress has a different terminal contract from throughput:
        // legacy orphan eviction is an observable rejection, not a hang.
        completion.begin_target(Instant::now());
        let started = Instant::now();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        let submission = runtime.block_on(submit_only(
            &controller,
            Arc::clone(&target),
            Arc::clone(&target_cycles),
            peers,
        ));
        let settled = submission.is_ok()
            && runtime.block_on(wait_for_stress_settlement(
                &completion,
                &relay_completion,
                deadline,
            ));
        let elapsed_ns = started.elapsed().as_nanos();
        resource_phases
            .capture("target_complete")
            .map_err(bench_error)?;
        #[cfg(not(feature = "cross-version-legacy-bench-adapter"))]
        {
            controller.stop();
            runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(30), async {
                    while controller.service_started() {
                        tokio::task::yield_now().await;
                    }
                })
                .await
            })?;
        }
        #[cfg(feature = "cross-version-legacy-bench-adapter")]
        broadcast_exit_signals();
        relay_guard.stop()?;
        resource_phases
            .capture("shutdown_complete")
            .map_err(bench_error)?;
        resource_phases.finish();
        let terminals = terminal_diagnostics(&completion, &relay_completion);
        let reset = relay_completion.generation_resets.load(Ordering::Acquire) != 0;
        println!(
            "BENCH_STRESS_RESULT {}",
            serde_json::json!({
                "schema_version": 1, "scenario": scenario, "contract": "capacity_recovery",
                "target": target_count, "warm": warm_count, "workers": workers, "peers": peers,
                "settled": settled, "submission_complete": submission.is_ok(), "exact_relay_history": !reset,
                "elapsed_ns": elapsed_ns, "terminals": terminals,
                "submission_error": submission.as_ref().err().map(ToString::to_string),
                "corpus": corpus,
            })
        );
        submission?;
        require(
            settled,
            "capacity stress left unresolved transactions; see BENCH_STRESS_RESULT",
        )?;
        let observation = terminal_diagnostics(&completion, &relay_completion);
        require(
            observation["accepted_rejected_overlap"] == 0
                && observation["unexpected_rejects"] == 0
                && observation["unexpected_callbacks"] == 0
                && observation["callback_duplicates"] == 0
                && observation["relay_duplicate_ok"] == 0
                && observation["relay_duplicate_reject"] == 0,
            "capacity stress terminal ownership violation; see BENCH_STRESS_RESULT",
        )?;
        relay_completion.validate_stress(&completion, &all_expected_relay, &allowed_unknown)?;
        diagnostics.armed = false;
        #[cfg(feature = "cross-version-legacy-bench-adapter")]
        {
            std::io::stdout().flush()?;
            std::process::exit(0)
        }
        #[cfg(not(feature = "cross-version-legacy-bench-adapter"))]
        return Ok(());
    }
    let reorg_snapshot = reorg_in_flight.then(|| {
        snapshot_with_proposed(
            &snapshot,
            &store,
            target.iter().map(TransactionView::proposal_short_id),
        )
    });
    runtime.block_on(submit_workload(
        &controller,
        &completion,
        warm,
        warm_cycles,
        warm_order,
        peers,
        0,
    ))?;
    runtime.block_on(
        relay_completion.wait_for_terminals(warm_expected_relay.len(), warm_expected_rejects.len()),
    )?;
    relay_completion.validate(
        &warm_expected_relay,
        &warm_expected_rejects,
        workload_scenario
            .ends_with("_reverse")
            .then_some(&warm_allowed_unknown),
    )?;
    completion.validate(warm_count, false)?;
    resource_phases
        .capture("warm_complete")
        .map_err(bench_error)?;
    let start_anchor = measurement_clock::ClockAnchor::capture().map_err(bench_error)?;
    begin_allocation_window();
    #[cfg(feature = "profiling")]
    if let Some(recorder) = observability.recorder.as_ref() {
        recorder
            .begin(|started| start_anchor.map(started))
            .map_err(std::io::Error::other)?;
    }
    let (target_user_cpu_started, target_system_cpu_started) = process_cpu_nanos()?;
    let started = Instant::now();
    completion.begin_target(started);
    let mut target_readiness = fanout_readiness::Readiness::default();
    let (reorg_latency_ns, reorg_overlap_callbacks) = if reorg_in_flight {
        let reorg_snapshot = reorg_snapshot.expect("reorg snapshot follows scenario identity");
        runtime.block_on(async {
            let submission = submit_batch(
                &controller,
                &completion,
                Arc::clone(&target),
                Arc::clone(&target_cycles),
                peers,
                warm_count + target_count,
            );
            let reorg = async {
                let overlap = completion.wait_for_callback_in_flight().await?;
                let started = Instant::now();
                controller
                    .update_tx_pool_for_reorg(
                        VecDeque::new(),
                        VecDeque::new(),
                        HashSet::new(),
                        reorg_snapshot,
                    )
                    .map_err(|error| std::io::Error::other(error.to_string()))?;
                Ok::<_, Box<dyn Error>>((started.elapsed().as_nanos(), overlap))
            };
            let (submission_result, reorg_result) = tokio::join!(submission, reorg);
            submission_result?;
            reorg_result
        })?
    } else {
        target_readiness = runtime.block_on(submit_workload(
            &controller,
            &completion,
            target,
            target_cycles,
            target_order,
            peers,
            warm_count,
        ))?;
        (0, 0)
    };
    runtime.block_on(
        relay_completion.wait_for_terminals(all_expected_relay.len(), all_expected_rejects.len()),
    )?;
    // Stop all measurement clocks at terminal completion. Exact set validation
    // and latency sorting remain mandatory, but are harness post-processing.
    let ended = Instant::now();
    let (target_user_cpu_ended, target_system_cpu_ended) = process_cpu_nanos()?;
    let (allocation_calls, allocated_bytes) = end_allocation_window();
    let end_anchor = measurement_clock::ClockAnchor::capture().map_err(bench_error)?;
    resource_phases
        .capture("target_complete")
        .map_err(bench_error)?;
    let elapsed = ended.duration_since(started);
    let target_user_cpu_ns = target_user_cpu_ended
        .checked_sub(target_user_cpu_started)
        .ok_or_else(|| std::io::Error::other("target user CPU clock moved backwards"))?;
    let target_system_cpu_ns = target_system_cpu_ended
        .checked_sub(target_system_cpu_started)
        .ok_or_else(|| std::io::Error::other("target system CPU clock moved backwards"))?;
    let target_cpu_ns = target_user_cpu_ns
        .checked_add(target_system_cpu_ns)
        .ok_or_else(|| std::io::Error::other("target-window process CPU time overflow"))?;
    let profile_window = start_anchor
        .window(&scenario, started, ended, &end_anchor)
        .map_err(bench_error)?;
    #[cfg(feature = "profiling")]
    if let Some(recorder) = observability.recorder.as_mut() {
        recorder
            .finish(&profile_window)
            .map_err(std::io::Error::other)?;
    }
    relay_completion.validate(
        &all_expected_relay,
        &all_expected_rejects,
        workload_scenario
            .ends_with("_reverse")
            .then_some(&allowed_unknown),
    )?;
    completion.validate(transactions.len(), reorg_in_flight)?;
    let p99_latency_ns = completion.end_target();
    resource_phases.capture("validated").map_err(bench_error)?;
    let reorg_latency_ns = if reorg_in_flight {
        reorg_latency_ns
    } else {
        let reorg_started = Instant::now();
        controller
            .update_tx_pool_for_reorg(
                VecDeque::new(),
                VecDeque::new(),
                HashSet::new(),
                Arc::clone(&snapshot),
            )
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        reorg_started.elapsed().as_nanos()
    };
    resource_phases
        .capture("reorg_complete")
        .map_err(bench_error)?;
    let shutdown_started = Instant::now();
    #[cfg(not(feature = "cross-version-legacy-bench-adapter"))]
    {
        controller.stop();
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(30), async {
                while controller.service_started() {
                    tokio::task::yield_now().await;
                }
            })
            .await
        })?;
    }
    #[cfg(feature = "cross-version-legacy-bench-adapter")]
    {
        broadcast_exit_signals();
    }
    relay_guard.stop()?;
    let shutdown_latency_ns = shutdown_started.elapsed().as_nanos();
    resource_phases
        .capture("shutdown_complete")
        .map_err(bench_error)?;
    resource_phases.finish();
    #[cfg(feature = "tokio-trace")]
    {
        // This diagnostic-only grace is outside all benchmark windows. It
        // gives a connected observer time to receive final task statistics;
        // receipt timestamps and dropped-event counts still qualify the data.
        std::thread::sleep(Duration::from_millis(250));
        observability
            .console
            .finish()
            .map_err(std::io::Error::other)?;
        println!("BENCH_DIAGNOSTICS tokio_console=true");
    }
    let throughput = target_count as f64 / elapsed.as_secs_f64();
    let accepted = completion.accepted_count();
    let callback_duplicates = completion.duplicate_callbacks.load(Ordering::Acquire);
    let relay = relay_completion.observation();
    let profile_observation = serde_json::json!({
        "schema_version": 2,
        "scenario": scenario,
        "target": target_count,
        "warm": warm_count,
        "workers": workers,
        "peers": peers,
        "elapsed_nanos": elapsed.as_nanos(),
        "throughput_tps": throughput,
        "accepted": accepted,
        "callback_duplicates": callback_duplicates,
        "p99_latency_nanos": p99_latency_ns,
        "target_cpu_nanos": target_cpu_ns,
        "target_user_cpu_nanos": target_user_cpu_ns,
        "target_system_cpu_nanos": target_system_cpu_ns,
        "allocation_calls": allocation_calls,
        "allocated_bytes": allocated_bytes,
        "reorg_latency_nanos": reorg_latency_ns,
        "reorg_overlap_callbacks": reorg_overlap_callbacks,
        "relay_ok": relay.ok,
        "relay_duplicate_ok": relay.duplicate_ok,
        "relay_rejects": relay.rejects,
        "relay_unknown_parents": relay.unknown_parents,
        "relay_unknown_parent_observations": relay.unknown_parent_observations,
        "relay_generation_resets": relay.generation_resets,
        "shutdown_latency_nanos": shutdown_latency_ns,
    });
    let adapter = if cfg!(feature = "cross-version-legacy-bench-adapter") {
        "legacy_peer_local_sequential"
    } else {
        "bounded_remote_batch"
    };
    println!(
        "BENCH_BUILD profiling={} allocation_observation={} callback_observer=preallocated_atomic_slots_sharded_completion adapter={} debug_assertions={} measurement_window=terminal_completion_v3 comparison_contract={}",
        cfg!(feature = "profiling"),
        cfg!(feature = "allocation-observation"),
        adapter,
        cfg!(debug_assertions),
        if omit_rbf_victim_notices {
            "rbf-victim-notice-ablation"
        } else {
            "protocol"
        },
    );
    println!("BENCH_CORPUS {corpus}");
    if matches!(target_order, SubmissionOrder::ReverseFanoutCohorts) {
        println!(
            "BENCH_READINESS {}",
            serde_json::json!({
                "schema_version": 1,
                "policy": "public_orphan_size_0_then_64_yield_v1",
                "query_count": target_readiness.queries,
                "completed_barriers": target_readiness.barriers,
                "elapsed_nanos": target_readiness.elapsed_nanos,
            })
        );
    }
    println!(
        "BENCH_TERMINALS {}",
        serde_json::json!({
            "callback_duplicates": callback_duplicates,
            "relay_ok": relay.ok,
            "relay_duplicate_ok": relay.duplicate_ok,
            "relay_rejects": relay.rejects,
            "relay_unknown_parent_observations": relay.unknown_parent_observations,
            "relay_generation_resets": relay.generation_resets,
        })
    );
    println!(
        "BENCH_RESULT scenario={scenario} target={target_count} warm={warm_count} workers={workers} peers={peers} elapsed_ns={} throughput_tps={throughput:.3} accepted={accepted} callback_duplicates={callback_duplicates} relay_ok={} relay_duplicate_ok={} relay_rejects={} relay_unknown_parents={} relay_generation_resets={} p99_latency_ns={p99_latency_ns} target_cpu_ns={target_cpu_ns} allocation_calls={allocation_calls} allocated_bytes={allocated_bytes} reorg_latency_ns={reorg_latency_ns} reorg_overlap_callbacks={reorg_overlap_callbacks} shutdown_latency_ns={shutdown_latency_ns}",
        elapsed.as_nanos(),
        relay.ok,
        relay.duplicate_ok,
        relay.rejects,
        relay.unknown_parents,
        relay.generation_resets,
    );
    println!("TX_POOL_PROFILE_OBSERVATION {profile_observation}");
    println!("TX_POOL_PROFILE_WINDOW {profile_window}");
    println!(
        "PROFILE_WINDOW start_unix_ns={} end_unix_ns={}",
        profile_window["start_unix_nanos"], profile_window["end_unix_nanos"]
    );
    diagnostics.armed = false;
    #[cfg(feature = "cross-version-legacy-bench-adapter")]
    {
        std::io::stdout().flush()?;
        std::process::exit(0)
    }
    #[cfg(not(feature = "cross-version-legacy-bench-adapter"))]
    Ok(())
}
