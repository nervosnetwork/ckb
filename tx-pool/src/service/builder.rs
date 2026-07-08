//! Tx-pool service builder.

use crate::block_assembler::{self, BlockAssembler};
use crate::callback::{Callbacks, PendingCallback, ProposedCallback, RejectCallback};
use crate::component::orphan::OrphanPool;
use crate::component::pipeline_queue::PipelineQueue;
use crate::component::recent_reject::RecentReject;
use crate::component::verify_queue::VerifyQueue;
use crate::constants::{
    DEFERRED_CHANNEL_SIZE, MESSAGE_CONCURRENCY_MULTIPLIER, PIPELINE_SHUTDOWN_TIMEOUT_SECONDS,
    SECONDS_PER_DAY,
};
use crate::network::{TxPoolNetwork, TxPoolNetworkHandle};
use crate::pool::TxPool;
use crate::service::{BLOCK_ASSEMBLER_CHANNEL_SIZE, DEFAULT_CHANNEL_SIZE};
use crate::service::{
    BlockAssemblerMessage, ChainReorgArgs, DeferredTask, Message, Notify, TxPoolController,
    TxPoolService, TxVerificationResult, process,
};
use crate::tx_source::TxSource;
use crate::util::panic_payload_to_string;
use crate::verify_mgr::VerifyMgr;
use ckb_app_config::{BlockAssemblerConfig, TxPoolConfig};
use ckb_async_runtime::Handle;
use ckb_fee_estimator::FeeEstimator;
use ckb_logger::{debug, error, info, warn};
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_stop_handler::new_tokio_exit_rx;
use ckb_types::core::TransactionView;
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
        let ordered_resolve_queue = Arc::new(RwLock::new(
            crate::component::ordered_resolve_queue::OrderedResolveQueue::new(),
        ));
        let verify_queue = Arc::new(RwLock::new(VerifyQueue::new(
            tx_pool_config.max_tx_verify_cycles,
            tx_pool_config.verify_ordering,
        )));

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
        let pre_check_cancel = signal_receiver.child_token();
        let pre_check_queue = Arc::new(crate::component::pre_check_queue::PreCheckQueue::new(
            pre_check_cancel.clone(),
        ));

        let (deferred_sender, deferred_receiver) =
            mpsc::channel::<DeferredTask>(DEFERRED_CHANNEL_SIZE);

        let service = TxPoolService {
            tx_pool_config: Arc::new(tx_pool.config.clone()),
            tx_pool: Arc::new(RwLock::new(tx_pool)),
            orphan: Arc::new(RwLock::new(OrphanPool::new())),
            block_assembler,
            txs_verify_cache,
            callbacks: Arc::new(callbacks),
            tx_relay_sender,
            block_assembler_sender,
            network,
            consensus,
            fee_estimator,
            recent_reject,
            queues: crate::component::pipeline_queues::PipelineQueues {
                ordered_resolve_queue: Arc::clone(&ordered_resolve_queue),
                verify_queue: Arc::clone(&verify_queue),
                pre_check_queue: Arc::clone(&pre_check_queue),
            },
            chunk_rx: chunk_rx.clone(),
            rbf_candidates: Arc::new(RwLock::new(
                crate::component::rbf_candidates::RbfCandidates::new(),
            )),
            deferred_sender,
        };

        let deferred_handle = spawn_deferred_worker(
            &handle,
            Arc::clone(&service.queues.ordered_resolve_queue),
            Arc::clone(&service.txs_verify_cache),
            deferred_receiver,
            signal_receiver.child_token(),
        );
        let pre_check_handles = Self::spawn_pre_check_workers(
            &handle,
            service.clone(),
            Arc::clone(&pre_check_queue),
            pre_check_cancel,
            pre_check_workers,
        );
        let verify_mgr_handle = Self::spawn_verify_mgr(
            &handle,
            service.clone(),
            chunk_rx.clone(),
            signal_receiver.clone(),
        );
        let resolver_handle = Self::spawn_resolver_monitor(
            &handle,
            service.clone(),
            Arc::clone(&ordered_resolve_queue),
            Arc::clone(&verify_queue),
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
        let reorg_handle = Self::spawn_reorg_handler(
            &handle,
            service.clone(),
            reorg_receiver,
            signal_receiver.clone(),
        );

        Self::spawn_message_dispatcher(
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

        started.store(true, Ordering::Release);
        if let Err(err) = tx_pool_controller.load_persisted_data(txs) {
            error!("Failed to import persistent txs, cause: {}", err);
        }
    }

    /// Spawn a pool of pre-check workers that pop jobs from the queue and
    /// classify them into the pipeline.  Returns the spawned task handles so
    /// the shutdown path can quiesce them before persisting.
    fn spawn_pre_check_workers(
        handle: &Handle,
        service: TxPoolService,
        pre_check_queue: Arc<crate::component::pre_check_queue::PreCheckQueue>,
        pre_check_cancel: CancellationToken,
        count: usize,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        let mut handles = Vec::with_capacity(count);
        for _ in 0..count {
            let svc = service.clone();
            let queue = Arc::clone(&pre_check_queue);
            let cancel = pre_check_cancel.child_token();
            let handle = handle.spawn(async move {
                loop {
                    let svc = svc.clone();
                    let queue = Arc::clone(&queue);
                    let worker = async move {
                        while let Some(job) = queue.pop().await {
                            let _ = svc.classify_and_enqueue_tx(job.tx, job.source).await;
                        }
                    };
                    let exit = match AssertUnwindSafe(worker).catch_unwind().await {
                        Ok(()) => crate::resolve_mgr::ResolveExit::Stopped,
                        Err(payload) => crate::resolve_mgr::ResolveExit::Panicked {
                            message: crate::util::panic_payload_to_string(payload.as_ref()),
                        },
                    };
                    if cancel.is_cancelled() {
                        break;
                    }
                    match exit {
                        crate::resolve_mgr::ResolveExit::Stopped => {
                            // Normal exit because the queue was cancelled.
                            break;
                        }
                        crate::resolve_mgr::ResolveExit::Panicked { message } => {
                            error!("tx-pool pre-check worker panicked: {}; respawning", message);
                        }
                    }
                }
            });
            handles.push(handle);
        }
        handles
    }

    /// Spawn the verification manager that supervises verify workers.  Returns
    /// the spawned task handle so the shutdown path can quiesce it before
    /// persisting.
    fn spawn_verify_mgr(
        handle: &Handle,
        service: TxPoolService,
        chunk_rx: watch::Receiver<ChunkCommand>,
        signal: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let mut verify_mgr = VerifyMgr::new(service, chunk_rx, signal);
        handle.spawn(async move { verify_mgr.run().await })
    }

    /// Spawn the ordered resolver monitor with panic-respawn protection.
    /// Returns the spawned task handle so the shutdown path can quiesce it
    /// before persisting.
    fn spawn_resolver_monitor(
        handle: &Handle,
        service: TxPoolService,
        ordered_resolve_queue: Arc<
            RwLock<crate::component::ordered_resolve_queue::OrderedResolveQueue>,
        >,
        verify_queue: Arc<RwLock<VerifyQueue>>,
        chunk_rx: watch::Receiver<ChunkCommand>,
        resolver_exit_signal: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        handle.spawn(async move {
            loop {
                let resolver = crate::resolve_mgr::OrderedResolver::new(
                    service.clone(),
                    Arc::clone(&ordered_resolve_queue),
                    Arc::clone(&verify_queue),
                    chunk_rx.clone(),
                    resolver_exit_signal.clone(),
                );
                let (exit_tx, mut exit_rx) = tokio::sync::mpsc::unbounded_channel();
                let handle = resolver.start(exit_tx);

                tokio::select! {
                    _ = resolver_exit_signal.cancelled() => {
                        let _ = handle.await;
                        break;
                    }
                    Some((_worker_id, exit)) = exit_rx.recv() => {
                        let _ = handle.await;
                        match exit {
                            crate::resolve_mgr::ResolveExit::Stopped => {
                                if resolver_exit_signal.is_cancelled() {
                                    break;
                                }
                                error!("tx-pool ordered resolver stopped unexpectedly, respawning");
                            }
                            crate::resolve_mgr::ResolveExit::Panicked { message } => {
                                error!("tx-pool ordered resolver panicked: {}; respawning", message);
                            }
                        }
                    }
                }
            }
            info!("TxPool ordered resolver monitor exited");
        })
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
        let max_workers = service.tx_pool_config.max_tx_verify_workers.max(1);
        let semaphore = Arc::new(Semaphore::new(max_workers * MESSAGE_CONCURRENCY_MULTIPLIER));
        handle.spawn(async move {
            loop {
                tokio::select! {
                    Some(message) = receiver.recv() => {
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
                            process(service_clone, message).await;
                        });
                    },
                    _ = signal_receiver.cancelled() => {
                        info!("TxPool is draining in-flight tasks...");
                        // Wait for all in-flight message-processing tasks to
                        // complete before persisting the pool state.  The
                        // semaphore bounds concurrent message handlers at
                        // max_workers * MESSAGE_CONCURRENCY_MULTIPLIER, so
                        // acquiring all permits guarantees no handler is still
                        // running.
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
                        break
                    },
                    else => break,
                }
            }
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

    /// Spawn the reorg handler with panic-respawn protection.
    fn spawn_reorg_handler(
        handle: &Handle,
        service: TxPoolService,
        mut reorg_receiver: mpsc::Receiver<Notify<ChainReorgArgs>>,
        signal_receiver: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        handle.spawn(async move {
            loop {
                let service = service.clone();
                let reorg_result = AssertUnwindSafe(async {
                    tokio::select! {
                        Some(message) = reorg_receiver.recv() => {
                            let Notify {
                                arguments: (detached_blocks, attached_blocks, detached_proposal_id, snapshot),
                            } = message;
                            let snapshot_clone = Arc::clone(&snapshot);
                            let detached_blocks_clone = detached_blocks.clone();
                            service.update_block_assembler_before_tx_pool_reorg(
                                detached_blocks_clone,
                                snapshot_clone
                            ).await;

                            let snapshot_clone = Arc::clone(&snapshot);
                            service
                            .update_tx_pool_for_reorg(
                                detached_blocks,
                                attached_blocks,
                                detached_proposal_id,
                                snapshot_clone,
                            )
                            .await;

                            service.update_block_assembler_after_tx_pool_reorg().await;
                            true
                        },
                        _ = signal_receiver.cancelled() => {
                            info!("TxPool reorg process service received exit signal, exit now");
                            false
                        },
                        else => false,
                    }
                })
                .catch_unwind()
                .await;

                match reorg_result {
                    Ok(true) => continue,
                    Ok(false) => break,
                    Err(payload) => {
                        let message = panic_payload_to_string(payload.as_ref());
                        error!("tx-pool reorg handler panicked: {}; respawning", message);
                        continue;
                    }
                }
            }
        })
    }

    /// Build a bare [`TxPoolService`] and its supporting queues **without**
    /// spawning any background workers (pre-check pool, [`VerifyMgr`],
    /// [`OrderedResolver`]).
    ///
    /// This is the single source of truth for service construction used by
    /// both [`Self::start`] (production) and the benchmark harness.  The
    /// caller is responsible for spawning the background workers that the
    /// returned service depends on.
    #[cfg(feature = "internal")]
    pub(crate) fn build_bench_service(self, network: TxPoolNetworkHandle) -> BenchServiceParts {
        let consensus = self.snapshot.cloned_consensus();

        let ordered_resolve_queue = Arc::new(RwLock::new(
            crate::component::ordered_resolve_queue::OrderedResolveQueue::new(),
        ));
        let verify_queue = Arc::new(RwLock::new(VerifyQueue::new(
            self.tx_pool_config.max_tx_verify_cycles,
            self.tx_pool_config.verify_ordering,
        )));

        let tx_pool = TxPool::new(self.tx_pool_config, self.snapshot);
        let recent_reject = self.recent_reject;
        let (block_assembler_sender, _) = self.block_assembler_channel;

        let signal = self.signal_receiver;
        let pre_check_cancel = signal.child_token();
        let pre_check_queue = Arc::new(crate::component::pre_check_queue::PreCheckQueue::new(
            pre_check_cancel,
        ));

        let (deferred_sender, deferred_receiver) =
            mpsc::channel::<DeferredTask>(DEFERRED_CHANNEL_SIZE);

        let service = TxPoolService {
            tx_pool_config: Arc::new(tx_pool.config.clone()),
            tx_pool: Arc::new(RwLock::new(tx_pool)),
            orphan: Arc::new(RwLock::new(OrphanPool::new())),
            block_assembler: self.block_assembler,
            txs_verify_cache: self.txs_verify_cache,
            callbacks: Arc::new(self.callbacks),
            tx_relay_sender: self.tx_relay_sender,
            block_assembler_sender,
            network,
            consensus,
            fee_estimator: self.fee_estimator,
            recent_reject,
            queues: crate::component::pipeline_queues::PipelineQueues {
                ordered_resolve_queue: Arc::clone(&ordered_resolve_queue),
                verify_queue: Arc::clone(&verify_queue),
                pre_check_queue: Arc::clone(&pre_check_queue),
            },
            chunk_rx: self.chunk_rx,
            rbf_candidates: Arc::new(RwLock::new(
                crate::component::rbf_candidates::RbfCandidates::new(),
            )),
            deferred_sender,
        };

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

/// Spawn the deferred task worker with panic-respawn protection.
///
/// Recovery tx re-enqueue and verify cache updates run sequentially in a
/// single background task. The worker only needs the ordered resolve queue
/// and the verify cache; it must NOT keep a clone of `TxPoolService`
/// (which holds `deferred_sender`), because the receiver task itself
/// holding a sender would keep the channel open forever.
pub(crate) fn spawn_deferred_worker(
    handle: &Handle,
    ordered_resolve_queue: Arc<
        RwLock<crate::component::ordered_resolve_queue::OrderedResolveQueue>,
    >,
    txs_verify_cache: Arc<RwLock<TxVerificationCache>>,
    deferred_receiver: mpsc::Receiver<DeferredTask>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    handle.spawn(async move {
        let mut deferred_rx = deferred_receiver;
        loop {
            // recv() is outside catch_unwind: the mpsc receiver is not
            // poisoned by panics in the message handler below.
            let task = tokio::select! {
                Some(task) = deferred_rx.recv() => task,
                _ = cancel.cancelled() => {
                    info!("deferred task worker received exit signal, exit now");
                    break;
                }
                else => break,
            };
            let ordered_resolve_queue = Arc::clone(&ordered_resolve_queue);
            let ordered_resolve_queue_retry = Arc::clone(&ordered_resolve_queue);
            let txs_verify_cache = Arc::clone(&txs_verify_cache);
            let recover_txs_for_retry = match &task {
                DeferredTask::RecoverTxs(txs) => Some(txs.clone()),
                DeferredTask::CacheUpdate { .. } => None,
            };
            let handler = async move {
                match task {
                    DeferredTask::RecoverTxs(txs) => {
                        enqueue_recover_txs(ordered_resolve_queue, txs).await;
                    }
                    DeferredTask::CacheUpdate { wtx_hash, verified } => {
                        let mut guard = txs_verify_cache.write().await;
                        guard.put(wtx_hash, verified);
                    }
                }
            };
            match AssertUnwindSafe(handler).catch_unwind().await {
                Ok(()) => {}
                Err(payload) => {
                    let message = panic_payload_to_string(payload.as_ref());
                    error!("deferred task worker panicked: {}; respawning", message);
                    // If a RecoverTxs task panicked, retry it directly
                    // without going back through the channel.
                    if let Some(txs) = recover_txs_for_retry {
                        enqueue_recover_txs(ordered_resolve_queue_retry, txs).await;
                    }
                }
            }
        }
        info!("deferred task worker exited (channel closed)");
    })
}

async fn enqueue_recover_txs(
    ordered_queue: Arc<RwLock<crate::component::ordered_resolve_queue::OrderedResolveQueue>>,
    txs: Vec<TransactionView>,
) {
    let mut queue = ordered_queue.write().await;
    for tx in txs {
        debug!("recover back: {:?}", tx.proposal_short_id());
        if let Err(reject) =
            queue.add_tx(crate::resolved_tx::ResolveJob::new(tx, TxSource::local()))
        {
            warn!(
                "failed to recover tx back to ordered resolve queue: {}",
                reject
            );
        }
    }
}
