//! Tx-pool service builder.

use crate::block_assembler::{self, BlockAssembler};
use crate::callback::{Callbacks, PendingCallback, ProposedCallback, RejectCallback};
use crate::component::recent_reject::RecentReject;
use crate::component::verify_queue::VerifyQueue;
use crate::component::waiting_room::WaitingRoom;
use crate::constants::{
    DEFERRED_CHANNEL_SIZE, MESSAGE_CONCURRENCY_MULTIPLIER, PIPELINE_SHUTDOWN_TIMEOUT_SECONDS,
    SECONDS_PER_DAY,
};
use crate::network::{TxPoolNetwork, TxPoolNetworkHandle};
use crate::pool::TxPool;
use crate::service::workers;
use crate::service::{BLOCK_ASSEMBLER_CHANNEL_SIZE, DEFAULT_CHANNEL_SIZE};
use crate::service::{
    BlockAssemblerMessage, ChainReorgArgs, DeferredTask, Message, Notify, TxPoolController,
    TxPoolService, TxVerificationResult, process,
};
use crate::util::panic_payload_to_string;
use ckb_app_config::{BlockAssemblerConfig, TxPoolConfig};
use ckb_async_runtime::Handle;
use ckb_fee_estimator::FeeEstimator;
use ckb_logger::{error, info, warn};
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_stop_handler::new_tokio_exit_rx;
use ckb_util::LinkedHashSet;
use ckb_verification::cache::TxVerificationCache;
use futures_util::FutureExt;

use std::panic::AssertUnwindSafe;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::{RwLock, Semaphore, mpsc, watch};
use tokio_util::sync::CancellationToken;

/// A builder used to create TxPoolService.
pub struct TxPoolServiceBuilder {
    pub(crate) tx_pool_config: TxPoolConfig,
    pub(crate) tx_pool_controller: TxPoolController,
    pub(crate) snapshot: Arc<Snapshot>,
    pub(crate) block_assembler: Option<BlockAssembler>,
    pub(crate) txs_verify_cache: Arc<RwLock<TxVerificationCache>>,
    pub(crate) callbacks: Callbacks,
    pub(crate) receiver: mpsc::Receiver<Message>,
    pub(crate) reorg_receiver: mpsc::Receiver<Notify<ChainReorgArgs>>,
    pub(crate) signal_receiver: CancellationToken,
    pub(crate) handle: Handle,
    pub(crate) tx_relay_sender: ckb_channel::Sender<TxVerificationResult>,
    pub(crate) chunk_rx: watch::Receiver<ChunkCommand>,
    pub(crate) started: Arc<AtomicBool>,
    pub(crate) block_assembler_channel: (
        mpsc::Sender<BlockAssemblerMessage>,
        mpsc::Receiver<BlockAssemblerMessage>,
    ),
    pub(crate) fee_estimator: FeeEstimator,
    pub(crate) recent_reject: Option<Arc<RecentReject>>,
}

/// Shared construction of the pipeline queues and a bare [`TxPoolService`],
/// used by both [`TxPoolServiceBuilder::start`] (production) and
/// [`TxPoolServiceBuilder::build_bench_service`] (internal benchmarks).
/// Spawning the background workers the service depends on is left to the
/// callers.
#[allow(clippy::too_many_arguments)]
fn assemble_service(
    tx_pool: TxPool,
    consensus: Arc<ckb_chain_spec::consensus::Consensus>,
    block_assembler: Option<BlockAssembler>,
    callbacks: Callbacks,
    network: TxPoolNetworkHandle,
    txs_verify_cache: Arc<RwLock<TxVerificationCache>>,
    recent_reject: Option<Arc<RecentReject>>,
    fee_estimator: FeeEstimator,
    tx_relay_sender: ckb_channel::Sender<TxVerificationResult>,
    block_assembler_sender: mpsc::Sender<BlockAssemblerMessage>,
    deferred_sender: mpsc::Sender<DeferredTask>,
    chunk_rx: watch::Receiver<ChunkCommand>,
    pre_check_cancel: CancellationToken,
) -> (
    TxPoolService,
    Arc<crate::component::pipeline_queues::PipelineQueues>,
) {
    // One `Arc` shared by the service and every worker: the queue fields
    // inside are plain locks, not per-queue `Arc`s.
    let queues = Arc::new(crate::component::pipeline_queues::PipelineQueues {
        ordered_resolve_queue: RwLock::new(
            crate::component::ordered_resolve_queue::OrderedResolveQueue::new(),
        ),
        verify_queue: RwLock::new(VerifyQueue::new(
            tx_pool.config.max_tx_verify_cycles,
            tx_pool.config.verify_ordering,
            tx_pool.config.verify_queue_tx_size_budget(),
        )),
        pre_check_queue: crate::component::pre_check_queue::PreCheckQueue::new(pre_check_cancel),
        rbf_candidates: RwLock::new(crate::component::rbf_candidates::RbfCandidates::new()),
    });

    let tx_pool_config = tx_pool.config.clone();
    let service = TxPoolService {
        pool: crate::service::PoolCore {
            tx_pool: Arc::new(RwLock::new(tx_pool)),
            consensus,
            tx_pool_config: Arc::new(tx_pool_config),
        },
        pipeline: crate::service::PipelineState {
            queues: Arc::clone(&queues),
            waiting_room: Arc::new(RwLock::new(WaitingRoom::new())),
            chunk_rx,
            deferred_sender,
        },
        relay: crate::service::RelayState {
            network,
            tx_relay_sender,
            block_assembler_sender,
            callbacks: Arc::new(callbacks),
            banned_peers: Default::default(),
        },
        aux: crate::service::AuxServices {
            txs_verify_cache,
            recent_reject,
            fee_estimator,
        },
        block_assembler,
        recovery_lock: Arc::new(tokio::sync::Mutex::new(())),
    };
    (service, queues)
}

impl TxPoolServiceBuilder {
    /// Creates a new TxPoolServiceBuilder.
    pub fn new(
        tx_pool_config: TxPoolConfig,
        snapshot: Arc<Snapshot>,
        block_assembler_config: Option<BlockAssemblerConfig>,
        txs_verify_cache: Arc<RwLock<TxVerificationCache>>,
        handle: &Handle,
        tx_relay_sender: ckb_channel::Sender<TxVerificationResult>,
        fee_estimator: FeeEstimator,
    ) -> (TxPoolServiceBuilder, TxPoolController) {
        let (sender, receiver) = mpsc::channel(DEFAULT_CHANNEL_SIZE);
        let block_assembler_channel = mpsc::channel(BLOCK_ASSEMBLER_CHANNEL_SIZE);
        let (reorg_sender, reorg_receiver) = mpsc::channel(DEFAULT_CHANNEL_SIZE);
        let signal_receiver: CancellationToken = new_tokio_exit_rx();
        let (chunk_tx, chunk_rx) = watch::channel(ChunkCommand::Resume);
        let started = Arc::new(AtomicBool::new(false));

        let controller = TxPoolController {
            sender,
            reorg_sender,
            handle: handle.clone(),
            chunk_tx: Arc::new(chunk_tx),
            started: Arc::clone(&started),
            signal: signal_receiver.clone(),
        };

        let block_assembler = block_assembler_config.and_then(|config| {
            BlockAssembler::new(config, Arc::clone(&snapshot))
                .inspect_err(|err| error!("failed to initialize block assembler: {}", err))
                .ok()
        });
        let recent_reject = Self::build_recent_reject(&tx_pool_config).map(Arc::new);
        let builder = TxPoolServiceBuilder {
            tx_pool_config,
            tx_pool_controller: controller.clone(),
            snapshot,
            block_assembler,
            txs_verify_cache,
            callbacks: Callbacks::new(),
            receiver,
            reorg_receiver,
            signal_receiver,
            handle: handle.clone(),
            tx_relay_sender,
            chunk_rx,
            started,
            block_assembler_channel,
            fee_estimator,
            recent_reject,
        };

        (builder, controller)
    }
}

/// Handles for the background workers that must quiesce before the pool is
/// persisted on graceful shutdown.
struct BackgroundWorkerHandles {
    deferred: tokio::task::JoinHandle<()>,
    pre_check: Vec<tokio::task::JoinHandle<()>>,
    verify_mgr: tokio::task::JoinHandle<()>,
    resolver: tokio::task::JoinHandle<()>,
    block_assembler: Option<tokio::task::JoinHandle<()>>,
    reorg: Option<tokio::task::JoinHandle<()>>,
}

impl BackgroundWorkerHandles {
    /// Wait for every background worker to finish concurrently, logging a
    /// warning if any of them does not exit within the supplied timeout.
    ///
    /// All workers are awaited in parallel so the total shutdown time is
    /// bounded by `timeout` rather than `N * timeout`.
    async fn quiesce(self, timeout: Duration) {
        let mut tasks: Vec<(String, tokio::task::JoinHandle<()>)> = Vec::new();
        tasks.push(("deferred worker".to_owned(), self.deferred));
        for (i, handle) in self.pre_check.into_iter().enumerate() {
            tasks.push((format!("pre-check worker {i}"), handle));
        }
        tasks.push(("verify manager".to_owned(), self.verify_mgr));
        tasks.push(("ordered resolver".to_owned(), self.resolver));
        if let Some(handle) = self.block_assembler {
            tasks.push(("block assembler loop".to_owned(), handle));
        }
        if let Some(handle) = self.reorg {
            tasks.push(("reorg handler".to_owned(), handle));
        }

        let results = tokio::time::timeout(
            timeout,
            futures_util::future::join_all(tasks.iter_mut().map(|(_, h)| h)),
        )
        .await;

        match results {
            Ok(_) => {}
            Err(_) => {
                for (label, handle) in &tasks {
                    if !handle.is_finished() {
                        warn!("{label} did not exit within shutdown timeout");
                    }
                }
            }
        }
    }
}

impl TxPoolServiceBuilder {
    /// Register new pending callback
    pub fn register_pending(&mut self, callback: PendingCallback) {
        self.callbacks.register_pending(callback);
    }

    /// Return cloned tx relayer sender
    pub fn tx_relay_sender(&self) -> ckb_channel::Sender<TxVerificationResult> {
        self.tx_relay_sender.clone()
    }

    /// Register new proposed callback
    pub fn register_proposed(&mut self, callback: ProposedCallback) {
        self.callbacks.register_proposed(callback);
    }

    /// Register new abandon callback
    pub fn register_reject(&mut self, callback: RejectCallback) {
        self.callbacks.register_reject(callback);
    }

    /// Access the shared recent-reject database (for registering callbacks).
    pub fn recent_reject(&self) -> Option<Arc<RecentReject>> {
        self.recent_reject.clone()
    }

    pub(crate) fn build_recent_reject(config: &TxPoolConfig) -> Option<RecentReject> {
        if !config.recent_reject.as_os_str().is_empty() {
            let recent_reject_ttl =
                u8::max(1, config.keep_rejected_tx_hashes_days) as i32 * SECONDS_PER_DAY;
            match RecentReject::new(
                &config.recent_reject,
                config.keep_rejected_tx_hashes_count,
                recent_reject_ttl,
            ) {
                Ok(recent_reject) => Some(recent_reject),
                Err(err) => {
                    error!(
                        "Failed to open the recent reject database {:?} {}",
                        config.recent_reject, err
                    );
                    None
                }
            }
        } else {
            warn!("Recent reject database is disabled!");
            None
        }
    }

    /// Start a background thread tx-pool service by taking ownership of the Builder, and returns a TxPoolController.
    pub fn start<N: TxPoolNetwork>(self, network: N) {
        // Production keeps the historical detached-service semantics. The
        // benchmark-only variant below retains this handle to await teardown.
        drop(self.start_inner(network));
    }

    /// Benchmark-only start variant that exposes the main dispatcher handle.
    /// Awaiting it after cancellation proves all message handlers and
    /// background workers have quiesced before the next benchmark iteration.
    #[cfg(feature = "internal")]
    pub(crate) fn start_with_handle<N: TxPoolNetwork>(
        self,
        network: N,
    ) -> tokio::task::JoinHandle<()> {
        self.start_inner(network)
    }

    fn start_inner<N: TxPoolNetwork>(self, network: N) -> tokio::task::JoinHandle<()> {
        if self.tx_pool_config.max_verify_queue_tx_size < self.tx_pool_config.max_tx_pool_size {
            warn!(
                "max_verify_queue_tx_size ({}) < max_tx_pool_size ({}): clamping the verify-queue \
                 budget up to max_tx_pool_size for burst/reload headroom",
                self.tx_pool_config.max_verify_queue_tx_size, self.tx_pool_config.max_tx_pool_size
            );
        }
        let network: TxPoolNetworkHandle = Arc::new(network);

        // Move all builder fields into locals so the rest of the startup sequence
        // can be expressed as a readable list of spawn calls without fighting the
        // borrow checker over partial moves.
        let Self {
            tx_pool_config,
            tx_pool_controller,
            snapshot,
            block_assembler,
            txs_verify_cache,
            callbacks,
            receiver,
            reorg_receiver,
            signal_receiver,
            handle,
            tx_relay_sender,
            chunk_rx,
            started,
            block_assembler_channel,
            fee_estimator,
            recent_reject,
        } = self;

        let consensus = snapshot.cloned_consensus();
        let pre_check_cancel = signal_receiver.child_token();

        let tx_pool = TxPool::new(tx_pool_config, snapshot);
        let txs = match tx_pool.load_from_file() {
            Ok(txs) => txs,
            Err(e) => {
                error!("{}", e.to_string());
                error!("Failed to load txs from tx-pool persistent data file, all txs are ignored");
                Vec::new()
            }
        };

        let (block_assembler_sender, block_assembler_receiver) = block_assembler_channel;
        let max_workers = tx_pool.config.max_tx_verify_workers.max(1);
        // Cap pre-check concurrency to the number of available CPU cores so that
        // cheap pre-resolution does not starve the heavier verification workers
        // on the shared tokio runtime.
        let pre_check_workers =
            max_workers.min(std::thread::available_parallelism().map_or(4, |n| n.get()));

        let (deferred_sender, deferred_receiver) =
            mpsc::channel::<DeferredTask>(DEFERRED_CHANNEL_SIZE);

        let (service, queues) = assemble_service(
            tx_pool,
            consensus,
            block_assembler,
            callbacks,
            network,
            txs_verify_cache,
            recent_reject,
            fee_estimator,
            tx_relay_sender,
            block_assembler_sender,
            deferred_sender,
            chunk_rx.clone(),
            pre_check_cancel.clone(),
        );

        let deferred_handle = workers::spawn_deferred_worker(
            &handle,
            Arc::clone(&queues),
            Arc::clone(&service.aux.txs_verify_cache),
            deferred_receiver,
            signal_receiver.child_token(),
            service.relay.clone(),
        );
        let pre_check_handles = workers::spawn_pre_check_workers(
            &handle,
            service.clone(),
            pre_check_cancel,
            pre_check_workers,
        );
        let verify_mgr_handle = workers::spawn_verify_mgr_monitor(
            &handle,
            service.clone(),
            chunk_rx.clone(),
            signal_receiver.clone(),
        );
        let resolver_handle = workers::spawn_resolver_monitor(
            &handle,
            service.clone(),
            chunk_rx,
            signal_receiver.child_token(),
        );
        let block_assembler_handle = service.block_assembler.as_ref().map(|block_assembler| {
            Self::spawn_block_assembler_loop(
                &handle,
                service.clone(),
                block_assembler.clone(),
                block_assembler_receiver,
                signal_receiver.clone(),
            )
        });
        let reorg_handle = workers::spawn_reorg_handler(
            &handle,
            service.clone(),
            reorg_receiver,
            signal_receiver.clone(),
        );

        let dispatcher_handle = Self::spawn_message_dispatcher(
            &handle,
            service,
            receiver,
            signal_receiver,
            BackgroundWorkerHandles {
                deferred: deferred_handle,
                pre_check: pre_check_handles,
                verify_mgr: verify_mgr_handle,
                resolver: resolver_handle,
                block_assembler: block_assembler_handle,
                reorg: Some(reorg_handle),
            },
        );

        if let Err(err) = tx_pool_controller.load_persisted_data(txs) {
            error!("Failed to import persistent txs, cause: {}", err);
        }
        // The builder owns an internal controller clone solely for startup
        // replay. Drop it before returning so the user-facing controllers are
        // the complete sender set; otherwise dropping all of them can never
        // close the dispatcher channel.
        drop(tx_pool_controller);
        started.store(true, Ordering::Release);
        dispatcher_handle
    }

    /// Spawn the main message dispatcher with bounded concurrency.
    fn spawn_message_dispatcher(
        handle: &Handle,
        service: TxPoolService,
        mut receiver: mpsc::Receiver<Message>,
        signal_receiver: CancellationToken,
        worker_handles: BackgroundWorkerHandles,
    ) -> tokio::task::JoinHandle<()> {
        let runtime_handle = handle.clone();
        let max_workers = service.pool.tx_pool_config.max_tx_verify_workers.max(1);
        let semaphore = Arc::new(Semaphore::new(max_workers * MESSAGE_CONCURRENCY_MULTIPLIER));
        handle.spawn(async move {
            loop {
                tokio::select! {
                    message = receiver.recv() => {
                        match message {
                            Some(message) => {
                                let service_clone = service.clone();
                                let permit = match Arc::clone(&semaphore).acquire_owned().await {
                                    Ok(permit) => permit,
                                    Err(_) => {
                                        info!("TxPool message dispatcher semaphore closed, exiting");
                                        break;
                                    }
                                };
                                runtime_handle.spawn(async move {
                                    let _permit = permit;
                                    // Message handlers must never take the whole
                                    // dispatcher down with a panic: catch and log,
                                    // matching the deferred worker's behaviour.
                                    let handler = process(service_clone, message);
                                    if let Err(payload) = AssertUnwindSafe(handler).catch_unwind().await {
                                        error!(
                                            "tx-pool message handler panicked: {}",
                                            panic_payload_to_string(payload.as_ref())
                                        );
                                    }
                                });
                            }
                            None => {
                                // Every sender dropped without an explicit stop.
                                // `select! else` cannot express this: the pending
                                // cancellation branch remains enabled forever,
                                // so an unmatched `Some(...)` pattern would never
                                // reach `else` after `recv()` returned `None`.
                                info!("TxPool message channel closed without shutdown signal");
                                break;
                            }
                        }
                    },
                    _ = signal_receiver.cancelled() => {
                        break
                    }
                }
            }

            // Idempotent for explicit cancellation; also covers every
            // defensive dispatcher exit such as a closed semaphore.
            signal_receiver.cancel();
            info!("TxPool is draining in-flight tasks...");
            // The semaphore bounds concurrent handlers, so acquiring every
            // permit proves all dispatched messages reached a stable outcome.
            let _ = semaphore
                .acquire_many(max_workers as u32 * MESSAGE_CONCURRENCY_MULTIPLIER as u32)
                .await;

            info!("TxPool is quiescing background workers...");
            worker_handles
                .quiesce(Duration::from_secs(PIPELINE_SHUTDOWN_TIMEOUT_SECONDS))
                .await;

            info!("TxPool is saving, please wait...");
            service.save_pool().await;
            info!("TxPool process_service exit now");
        })
    }

    /// Spawn the block assembler message loop.
    fn spawn_block_assembler_loop(
        handle: &Handle,
        service: TxPoolService,
        block_assembler: BlockAssembler,
        mut block_assembler_receiver: mpsc::Receiver<BlockAssemblerMessage>,
        signal_receiver: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let interval = Duration::from_millis(block_assembler.config.update_interval_millis);
        if interval.is_zero() {
            // block_assembler.update_interval_millis set zero interval should only be used for tests,
            // external notification will be disabled.
            ckb_logger::warn!(
                "block_assembler.update_interval_millis set to zero interval. \
                This should only be used for tests, as external notification will be disabled."
            );
            handle.spawn(async move {
                loop {
                    tokio::select! {
                        Some(message) = block_assembler_receiver.recv() => {
                            let service_clone = service.clone();
                            block_assembler::process(service_clone, &message).await;
                        },
                        _ = signal_receiver.cancelled() => {
                            info!("TxPool block_assembler process service received exit signal, exit now");
                            break
                        },
                        else => break,
                    }
                }
            })
        } else {
            handle.spawn(async move {
                let mut interval = tokio::time::interval(interval);
                let mut queue = LinkedHashSet::new();
                loop {
                    tokio::select! {
                        Some(message) = block_assembler_receiver.recv() => {
                            if let BlockAssemblerMessage::Reset(..) = message {
                                let service_clone = service.clone();
                                queue.clear();
                                block_assembler::process(service_clone, &message).await;
                            } else {
                                queue.insert(message);
                            }
                        },
                        _ = interval.tick() => {
                            for message in &queue {
                                let service_clone = service.clone();
                                block_assembler::process(service_clone, message).await;
                            }
                            if !queue.is_empty()
                                && let Some(ref block_assembler) = service.block_assembler {
                                    block_assembler.notify().await;
                                }
                            queue.clear();
                        }
                        _ = signal_receiver.cancelled() => {
                            info!("TxPool block_assembler process service received exit signal, exit now");
                            break
                        },
                        else => break,
                    }
                }
            })
        }
    }

    /// Build a bare [`TxPoolService`] and its supporting queues **without**
    /// spawning any background workers (pre-check pool, [`VerifyMgr`],
    /// [`OrderedResolver`]).
    ///
    /// Shares the exact same construction as [`Self::start`] via
    /// `assemble_service`; the caller is responsible for spawning the
    /// background workers that the returned service depends on.
    #[cfg(feature = "internal")]
    pub(crate) fn build_bench_service(self, network: TxPoolNetworkHandle) -> BenchServiceParts {
        let consensus = self.snapshot.cloned_consensus();
        let signal = self.signal_receiver;
        let pre_check_cancel = signal.child_token();

        let tx_pool = TxPool::new(self.tx_pool_config, self.snapshot);
        let (block_assembler_sender, _) = self.block_assembler_channel;
        let (deferred_sender, deferred_receiver) =
            mpsc::channel::<DeferredTask>(DEFERRED_CHANNEL_SIZE);

        let (service, _queues) = assemble_service(
            tx_pool,
            consensus,
            self.block_assembler,
            self.callbacks,
            network,
            self.txs_verify_cache,
            self.recent_reject,
            self.fee_estimator,
            self.tx_relay_sender,
            block_assembler_sender,
            deferred_sender,
            self.chunk_rx,
            pre_check_cancel,
        );

        BenchServiceParts {
            service,
            signal,
            deferred_receiver,
        }
    }
}

/// Components returned by [`TxPoolServiceBuilder::build_bench_service`].
///
/// The caller must spawn the pre-check workers, [`VerifyMgr`], and
/// [`OrderedResolver`] using these components.
#[cfg(feature = "internal")]
pub(crate) struct BenchServiceParts {
    pub service: TxPoolService,
    pub signal: CancellationToken,
    pub deferred_receiver: mpsc::Receiver<DeferredTask>,
}
