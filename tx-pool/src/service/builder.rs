//! Tx-pool service builder.

use crate::block_assembler::{self, BlockAssembler};
use crate::callback::{Callbacks, PendingCallback, ProposedCallback, RejectCallback};
use crate::component::recent_reject::RecentReject;
use crate::constants::{
    EFFECT_JOURNAL_REMOTE_MAX_BATCHES, EFFECT_TRUSTED_HEADROOM_BATCHES,
    MESSAGE_CONCURRENCY_MULTIPLIER, PIPELINE_SHUTDOWN_TIMEOUT_SECONDS, SECONDS_PER_DAY,
    VERIFY_CACHE_CHANNEL_SIZE,
};
use crate::network::{TxPoolNetwork, TxPoolNetworkHandle};
use crate::pool::TxPool;
use crate::service::effects::{
    EffectEndpoints, EffectJournal, max_pool_mutation_effect_bytes, max_submit_effect_bytes,
    run_effect_publisher,
};
use crate::service::workers;
use crate::service::{BLOCK_ASSEMBLER_CHANNEL_SIZE, DEFAULT_CHANNEL_SIZE, REORG_CHANNEL_SIZE};
use crate::service::{
    BlockAssemblerMessage, ChainReorgArgs, Message, Notify, TxPoolController, TxPoolService,
    TxVerificationResult, VerifyCacheUpdate, process,
};
use ckb_app_config::{BlockAssemblerConfig, TxPoolConfig};
use ckb_async_runtime::Handle;
use ckb_fee_estimator::FeeEstimator;
use ckb_logger::{error, info, warn};
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_stop_handler::new_tokio_exit_rx;
use ckb_util::LinkedHashSet;
use ckb_verification::cache::TxVerificationCache;
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

/// Shared construction of the pre-pool runtime and a bare [`TxPoolService`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn assemble_service(
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
    verify_cache_sender: mpsc::Sender<VerifyCacheUpdate>,
    pipeline_shutdown: CancellationToken,
) -> TxPoolService {
    let kernel = Arc::new(crate::component::pre_pool::PrePool::new(
        &tx_pool.config,
        &consensus,
        pipeline_shutdown,
    ));
    let tx_pool_config = tx_pool.config.clone();
    let banned_peer_capacity = kernel
        .max_entries()
        .saturating_add(DEFAULT_CHANNEL_SIZE)
        .saturating_add(
            tx_pool_config
                .max_tx_verify_workers
                .max(1)
                .saturating_mul(MESSAGE_CONCURRENCY_MULTIPLIER),
        );
    // Static effect regions: Remote owns the ordinary ceiling, trusted work
    // has one largest-admission byte cohort plus fixed batch headroom, and
    // chain authority has one independent reorg cohort. None is a dynamic
    // reservation and Remote traffic cannot consume either higher class.
    let resident_effect_bytes = tx_pool_config
        .max_tx_pool_size
        .saturating_add(tx_pool_config.tx_pipeline_resident_size_budget())
        .saturating_mul(2);
    let submit_effect_bytes = max_submit_effect_bytes(
        tx_pool_config.max_tx_pool_size,
        consensus.max_block_bytes() as usize,
    );
    let reorg_effect_bytes =
        max_pool_mutation_effect_bytes(tx_pool_config.max_tx_pool_size).max(4096);
    let ordinary_effect_bytes = resident_effect_bytes.max(submit_effect_bytes);
    let effects = Arc::new(
        EffectJournal::new_partitioned(
            EFFECT_JOURNAL_REMOTE_MAX_BATCHES,
            ordinary_effect_bytes,
            EFFECT_TRUSTED_HEADROOM_BATCHES,
            submit_effect_bytes,
            1,
            reorg_effect_bytes,
        )
        .unwrap_or_else(|error| panic!("failed to allocate tx-pool effect journal: {error:?}")),
    );
    TxPoolService {
        pool: crate::service::PoolCore {
            tx_pool: Arc::new(RwLock::new(tx_pool)),
            consensus,
            tx_pool_config: Arc::new(tx_pool_config),
        },
        pipeline: crate::service::PipelineState {
            kernel,
            epoch: Arc::new(crate::service::PipelineEpoch::default()),
            verify_cache_sender,
        },
        relay: crate::service::RelayState {
            network,
            tx_relay_sender,
            block_assembler_sender,
            block_assembler_dirty: Arc::new(Default::default()),
            block_assembler_reset: Arc::new(Default::default()),
            callbacks: Arc::new(callbacks),
            effects,
            banned_peers: Arc::new(crate::service::BannedPeerSet::new(banned_peer_capacity)),
        },
        aux: crate::service::AuxServices {
            txs_verify_cache,
            recent_reject,
            fee_estimator,
        },
        block_assembler,
        persistence_lock: Arc::new(tokio::sync::Mutex::new(())),
    }
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
        let (reorg_sender, reorg_receiver) = mpsc::channel(REORG_CHANNEL_SIZE);
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
    effects: tokio::task::JoinHandle<()>,
    verify_cache: tokio::task::JoinHandle<()>,
    maintenance: tokio::task::JoinHandle<()>,
    commit: tokio::task::JoinHandle<()>,
    pre_check: Vec<tokio::task::JoinHandle<()>>,
    verify: Vec<tokio::task::JoinHandle<()>>,
    resolver: tokio::task::JoinHandle<()>,
    block_assembler: Option<tokio::task::JoinHandle<()>>,
    reorg: Option<tokio::task::JoinHandle<()>>,
}

/// Rust-native supervision bridge for detached concurrent message handlers.
/// A normal return disarms the guard. Unwinding drops it armed, marks the
/// generation ineligible for persistence, and asks the owning dispatcher to
/// quiesce every state worker. It does not catch, retry, or repair the panic.
struct MessageHandlerGuard {
    shutdown: CancellationToken,
    failed: Arc<AtomicBool>,
    completed: bool,
}

impl MessageHandlerGuard {
    fn new(shutdown: CancellationToken, failed: Arc<AtomicBool>) -> Self {
        Self {
            shutdown,
            failed,
            completed: false,
        }
    }

    fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for MessageHandlerGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.failed.store(true, Ordering::Release);
            self.shutdown.cancel();
        }
    }
}

impl BackgroundWorkerHandles {
    fn any_finished(&self) -> bool {
        self.effects.is_finished()
            || self.verify_cache.is_finished()
            || self.maintenance.is_finished()
            || self.commit.is_finished()
            || self.pre_check.iter().any(|handle| handle.is_finished())
            || self.verify.iter().any(|handle| handle.is_finished())
            || self.resolver.is_finished()
            || self
                .block_assembler
                .as_ref()
                .is_some_and(|handle| handle.is_finished())
            || self
                .reorg
                .as_ref()
                .is_some_and(|handle| handle.is_finished())
    }

    /// Wait for every background worker to finish concurrently, logging a
    /// warning if any of them does not exit within the supplied timeout.
    ///
    /// All workers are awaited in parallel so the total shutdown time is
    /// bounded by `timeout` rather than `N * timeout`.
    async fn quiesce(self, timeout: Duration, effect_journal: &EffectJournal) -> bool {
        let mut effect_publisher = self.effects;
        let mut tasks: Vec<(String, tokio::task::JoinHandle<()>)> = Vec::new();
        tasks.push(("verify-cache worker".to_owned(), self.verify_cache));
        tasks.push(("pipeline maintenance".to_owned(), self.maintenance));
        tasks.push(("pipeline commit worker".to_owned(), self.commit));
        for (i, handle) in self.pre_check.into_iter().enumerate() {
            tasks.push((format!("pre-check worker {i}"), handle));
        }
        for (i, handle) in self.verify.into_iter().enumerate() {
            tasks.push((format!("verify worker {i}"), handle));
        }
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

        let mut state_workers_clean = true;
        match results {
            Ok(results) => {
                for ((label, _), result) in tasks.iter().zip(results) {
                    if let Err(error) = result {
                        warn!("{label} did not exit cleanly: {error}");
                        state_workers_clean = false;
                    }
                }
            }
            Err(_) => {
                for (label, handle) in &tasks {
                    if !handle.is_finished() {
                        warn!("{label} did not exit within shutdown timeout");
                        handle.abort();
                    }
                }
                // A timed-out reorg may be between the snapshot swap and
                // detached-transaction recovery. Never persist that partial
                // point. Close/abort publication so shutdown remains bounded;
                // the next start rebuilds from the last complete pool file.
                effect_journal.close();
                effect_publisher.abort();
                return false;
            }
        }

        // No state worker can enqueue after this point. Close only now, then
        // drain every stable-state effect before persistence and service exit.
        effect_journal.close();
        let effect_publisher_clean =
            match tokio::time::timeout(timeout, &mut effect_publisher).await {
                Ok(Ok(())) => true,
                Ok(Err(error)) => {
                    warn!("effect publisher did not exit cleanly: {error}");
                    false
                }
                Err(_) => {
                    warn!("effect publisher did not drain within shutdown timeout; aborting it");
                    effect_publisher.abort();
                    false
                }
            };
        state_workers_clean && effect_publisher_clean
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

    /// Test/benchmark start variant that exposes the main dispatcher handle.
    /// Awaiting it after cancellation proves all message handlers and
    /// background workers have quiesced before persistence or the next
    /// benchmark iteration.
    #[cfg(any(test, feature = "internal"))]
    pub(crate) fn start_with_handle<N: TxPoolNetwork>(
        self,
        network: N,
    ) -> tokio::task::JoinHandle<()> {
        self.start_inner(network)
    }

    fn start_inner<N: TxPoolNetwork>(self, network: N) -> tokio::task::JoinHandle<()> {
        if self.tx_pool_config.max_tx_pool_resident_size < self.tx_pool_config.max_tx_pool_size {
            warn!(
                "max_tx_pool_resident_size ({}) < max_tx_pool_size ({}): clamping the accepted-pool residency budget up to max_tx_pool_size",
                self.tx_pool_config.max_tx_pool_resident_size, self.tx_pool_config.max_tx_pool_size
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
        let txs = match tx_pool.load_persistence_snapshot() {
            Ok(snapshot) => snapshot.into_transactions(),
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

        let (verify_cache_sender, verify_cache_receiver) =
            mpsc::channel::<VerifyCacheUpdate>(VERIFY_CACHE_CHANNEL_SIZE);

        let service = assemble_service(
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
            verify_cache_sender,
            signal_receiver.clone(),
        );

        let verify_cache_handle = workers::spawn_verify_cache_worker(
            &handle,
            Arc::clone(&service.aux.txs_verify_cache),
            verify_cache_receiver,
            signal_receiver.child_token(),
        );
        let effect_handle = handle.spawn(run_effect_publisher(
            Arc::clone(&service.relay.effects),
            EffectEndpoints {
                network: Arc::clone(&service.relay.network),
                tx_relay_sender: service.relay.tx_relay_sender.clone(),
            },
        ));
        let maintenance_handle = workers::spawn_pipeline_maintenance_worker(
            &handle,
            service.clone(),
            signal_receiver.child_token(),
        );
        let commit_handle = workers::spawn_pipeline_commit_worker(
            &handle,
            service.clone(),
            signal_receiver.child_token(),
        );
        let pre_check_handles = workers::spawn_pre_check_workers(
            &handle,
            service.clone(),
            pre_check_cancel,
            pre_check_workers,
        );
        let verify_handles = crate::verify_mgr::spawn_verify_workers(
            &handle,
            service.clone(),
            chunk_rx.clone(),
            signal_receiver.child_token(),
        );
        let resolver_handle = crate::resolve_mgr::spawn_ordered_resolver(
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
                effects: effect_handle,
                verify_cache: verify_cache_handle,
                maintenance: maintenance_handle,
                commit: commit_handle,
                pre_check: pre_check_handles,
                verify: verify_handles,
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
        let message_concurrency = max_workers
            .checked_mul(MESSAGE_CONCURRENCY_MULTIPLIER)
            .and_then(|permits| u32::try_from(permits).ok())
            .expect("tx_pool.max_tx_verify_workers exceeds dispatcher permit capacity");
        let semaphore = Arc::new(Semaphore::new(message_concurrency as usize));
        handle.spawn(async move {
            let handler_failed = Arc::new(AtomicBool::new(false));
            let mut worker_failed = false;
            let mut supervisor = tokio::time::interval_at(
                tokio::time::Instant::now() + Duration::from_millis(100),
                Duration::from_millis(100),
            );
            supervisor.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
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
                                let handler_shutdown = signal_receiver.clone();
                                let handler_failed = Arc::clone(&handler_failed);
                                runtime_handle.spawn(async move {
                                    let _permit = permit;
                                    let mut guard = MessageHandlerGuard::new(
                                        handler_shutdown,
                                        handler_failed,
                                    );
                                    process(service_clone, message).await;
                                    guard.complete();
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
                    },
                    _ = supervisor.tick() => {
                        if !signal_receiver.is_cancelled() && worker_handles.any_finished() {
                            error!("tx-pool background worker exited unexpectedly; shutting down");
                            worker_failed = true;
                            break;
                        }
                    }
                }
            }

            // Idempotent for explicit cancellation; also covers every
            // defensive dispatcher exit such as a closed semaphore.
            signal_receiver.cancel();
            // Stop accepting controller re-entry before waiting for active
            // handlers. A handler may invoke a user callback that
            // synchronously sends another controller request and waits for
            // its response; if the receiver stayed open while the dispatcher
            // no longer polled it, shutdown would wait forever for that
            // handler. Drop already-buffered requests too so their responders
            // fail promptly instead of waiting until persistence completes.
            receiver.close();
            while receiver.try_recv().is_ok() {}
            info!("TxPool is draining in-flight tasks...");
            // The semaphore bounds concurrent handlers, so acquiring every
            // permit proves all dispatched messages reached a stable outcome.
            let _ = semaphore
                .acquire_many(message_concurrency)
                .await;

            info!("TxPool is quiescing background workers...");
            let workers_clean = worker_handles
                .quiesce(
                    Duration::from_secs(PIPELINE_SHUTDOWN_TIMEOUT_SECONDS),
                    &service.relay.effects,
                )
                .await;
            let clean_shutdown = workers_clean
                && !worker_failed
                && !handler_failed.load(Ordering::Acquire);

            if clean_shutdown {
                info!("TxPool is saving, please wait...");
                service.save_pool().await;
            } else {
                warn!(
                    "TxPool shutdown did not reach a complete recovery/effect boundary; skipping persistence"
                );
            }
            info!("TxPool process_service exit now");
        })
    }

    /// Apply loaded template work and acknowledge only generations that
    /// reached a coherent template. Failed messages stay both in the local
    /// queue and in the authoritative journal; a racing newer generation is
    /// preserved by conditional acknowledgement.
    pub(crate) async fn apply_block_assembler_updates(
        service: &TxPoolService,
        queue: &mut LinkedHashSet<BlockAssemblerMessage>,
    ) -> bool {
        let dirty = service.relay.load_block_assembler_dirty();
        for (message, _) in &dirty {
            queue.insert(message.clone());
        }

        let attempted = std::mem::take(queue);
        let mut retry = LinkedHashSet::new();
        let mut made_progress = false;
        for message in attempted {
            if block_assembler::process(service.clone(), &message).await {
                if let Some((_, generation)) = dirty
                    .iter()
                    .find(|(dirty_message, _)| dirty_message == &message)
                {
                    service
                        .relay
                        .complete_block_assembler_dirty(&message, *generation);
                }
                made_progress = true;
            } else {
                retry.insert(message);
            }
        }
        *queue = retry;
        made_progress
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
        let eager_updates = interval.is_zero();
        if eager_updates {
            ckb_logger::warn!(
                "block_assembler.update_interval_millis set to zero interval. \
                This should only be used for tests, as external notification will be disabled."
            );
        }
        handle.spawn(async move {
            // Interval-zero mode still retries authoritative resets, but it
            // applies ordinary updates eagerly and deliberately emits no
            // external miner notification.
            let tick_period = if eager_updates {
                Duration::from_secs(1)
            } else {
                interval
            };
            let first_tick = if eager_updates {
                tokio::time::Instant::now() + tick_period
            } else {
                tokio::time::Instant::now()
            };
            let mut ticker = tokio::time::interval_at(first_tick, tick_period);
            let mut queue = LinkedHashSet::new();
            loop {
                tokio::select! {
                    Some(message) = block_assembler_receiver.recv() => {
                        if !matches!(message, BlockAssemblerMessage::Reset) {
                            queue.insert(message);
                        }
                        // Reset is the hard template barrier and may have
                        // shared a saturated channel with any wake token.
                        block_assembler::process(
                            service.clone(),
                            &BlockAssemblerMessage::Reset,
                        )
                        .await;
                        if eager_updates && !service.relay.block_assembler_reset_pending() {
                            Self::apply_block_assembler_updates(&service, &mut queue).await;
                        }
                    },
                    _ = ticker.tick() => {
                        if eager_updates && !service.relay.block_assembler_reset_pending() {
                            continue;
                        }
                        block_assembler::process(
                            service.clone(),
                            &BlockAssemblerMessage::Reset,
                        )
                        .await;
                        if service.relay.block_assembler_reset_pending() {
                            continue;
                        }
                        let made_progress =
                            Self::apply_block_assembler_updates(&service, &mut queue).await;
                        if !eager_updates && made_progress {
                            block_assembler.notify().await;
                        }
                    },
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

#[cfg(test)]
#[path = "tests/builder_shutdown.rs"]
mod shutdown_tests;
