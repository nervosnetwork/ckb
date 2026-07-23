//! Background worker spawning for the tx-pool pipeline.
//!
//! Every long-running pipeline task is spawned here: the pre-check worker
//! pool, the verify-manager and ordered-resolver monitors (with
//! panic-respawn backoff), the reorg handler, and the deferred-task worker.
//! The service builder (`service::builder`) keeps only assembly, startup
//! and shutdown orchestration; worker lifecycle lives in this module.

use crate::service::{ChainReorgArgs, DeferredTask, Notify, TxPoolService};
use crate::verify_mgr::VerifyMgr;
use ckb_async_runtime::Handle;
use ckb_logger::{error, info};
use ckb_script::ChunkCommand;
use futures_util::FutureExt;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::util::panic_payload_to_string;

/// Backoff between respawns of a crashed worker monitor.
///
/// Respawning immediately is right for a worker that died after a long
/// healthy run (a rare panic), while a persistent start-time failure (a
/// panic that fires immediately on every run) must not become a hot spin
/// with log spam. The delay therefore doubles per consecutive failure
/// (100ms → 25.6s, capped at 30s) and resets to the base after any run
/// that stayed up for at least `HEALTHY_RUN`.
struct RespawnBackoff {
    failures: u32,
}

impl RespawnBackoff {
    /// First retry delay after a failure.
    const BASE: Duration = Duration::from_millis(100);
    /// Maximum delay between respawns.
    const MAX: Duration = Duration::from_secs(30);
    /// A run lasting at least this long counts as healthy and resets the
    /// backoff to `BASE`.
    const HEALTHY_RUN: Duration = Duration::from_secs(60);

    fn new() -> Self {
        Self { failures: 0 }
    }

    /// Delay before the next respawn, given how long the previous run
    /// lasted.
    fn delay_for(&mut self, ran_for: Duration) -> Duration {
        if ran_for >= Self::HEALTHY_RUN {
            self.failures = 0;
        }
        let delay = Self::BASE.saturating_mul(2u32.saturating_pow(self.failures.min(10)));
        self.failures = self.failures.saturating_add(1);
        delay.min(Self::MAX)
    }
}

/// Retain one ordered state-transition message until it completes or the
/// service is shutting down. A deterministic panic is backoff-limited but can
/// never turn into an acknowledged/dropped message.
async fn retry_retained_message<T, F, Fut>(
    worker_name: &'static str,
    item: T,
    cancel: &CancellationToken,
    mut handler: F,
) -> bool
where
    T: Clone,
    F: FnMut(T) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut backoff = RespawnBackoff::new();
    loop {
        if cancel.is_cancelled() {
            return false;
        }
        let started = std::time::Instant::now();
        match crate::worker::catch_job_panic(handler(item.clone())).await {
            Ok(()) => return true,
            Err(message) => {
                let delay = backoff.delay_for(started.elapsed());
                error!(
                    "{} panicked; retaining head message and retrying in {:?}: {}",
                    worker_name, delay, message
                );
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = cancel.cancelled() => return false,
                }
            }
        }
    }
}

/// Consume an ordered channel without acknowledging the head item until its
/// handler succeeds. Later messages stay in the bounded channel, providing
/// backpressure and preserving transition order.
async fn run_retained_receiver<T, F, Fut>(
    worker_name: &'static str,
    mut receiver: mpsc::Receiver<T>,
    cancel: &CancellationToken,
    mut handler: F,
) where
    T: Clone,
    F: FnMut(T) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    loop {
        let item = tokio::select! {
            item = receiver.recv() => item,
            _ = cancel.cancelled() => None,
        };
        let Some(item) = item else {
            break;
        };
        if !retry_retained_message(worker_name, item, cancel, &mut handler).await {
            break;
        }
    }
}

/// The pre-check worker body. Ownership moves queued → active → resolved,
/// waiting, or terminal entirely inside the coordinator; there is no trailing
/// `finish` call that a stale worker could apply to a newer incarnation.
pub(crate) async fn run_pre_check_worker_loop(service: TxPoolService) {
    loop {
        match service
            .pipeline
            .runtime
            .wait_raw(crate::component::pipeline_coordinator::RawStage::PreCheck)
            .await
        {
            Ok(Some(lease)) => service.process_pipeline_raw_lease(lease).await,
            Ok(None) => break,
            Err(error) => {
                error!("tx-pool pre-check checkout failed: {:?}", error);
                tokio::task::yield_now().await;
            }
        }
    }
}

/// Spawn a pool of pre-check workers that pop jobs from the queue and
/// classify them into the pipeline.  Returns the spawned task handles so
/// the shutdown path can quiesce them before persisting.
pub(crate) fn spawn_pre_check_workers(
    handle: &Handle,
    service: TxPoolService,
    pre_check_cancel: CancellationToken,
    count: usize,
) -> Vec<tokio::task::JoinHandle<()>> {
    let mut handles = Vec::with_capacity(count);
    for _ in 0..count {
        let svc = service.clone();
        let cancel = pre_check_cancel.child_token();
        let handle = handle.spawn(async move {
            let mut backoff = RespawnBackoff::new();
            loop {
                let svc = svc.clone();
                let started = std::time::Instant::now();
                let worker = run_pre_check_worker_loop(svc);
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
                        tokio::select! {
                            _ = tokio::time::sleep(backoff.delay_for(started.elapsed())) => {}
                            _ = cancel.cancelled() => break,
                        }
                    }
                }
            }
        });
        handles.push(handle);
    }
    handles
}

/// Drain coordinator cascades, conflict rechecks and remote expiry in bounded
/// slices. The notification is level-triggered for graph work; a coarse timer
/// is retained only for wall-clock expiry. No slice can grow with the full
/// attacker-controlled graph while holding the coordinator mutex.
pub(crate) fn spawn_pipeline_maintenance_worker(
    handle: &Handle,
    service: TxPoolService,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    const SLICE: usize = 32;
    const EXPIRY_TICK: Duration = Duration::from_secs(1);

    handle.spawn(async move {
        let ready = service.pipeline.runtime.subscribe_maintenance();
        let mut expiry = tokio::time::interval(EXPIRY_TICK);
        expiry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = ready.notified() => {}
                _ = expiry.tick() => {}
                _ = cancel.cancelled() => break,
            }

            loop {
                let now = ckb_systemtime::unix_time().as_secs();
                let expiry_permit = match service
                    .reserve_effects(TxPoolService::pipeline_terminal_effect_bytes(SLICE))
                    .await
                {
                    Ok(permit) => permit,
                    Err(error) => {
                        error!("tx-pool expiry effect reservation failed: {:?}", error);
                        break;
                    }
                };
                let expired = match service.pipeline.runtime.mutate(|coordinator| {
                    let result = coordinator.expire_due(now, SLICE);
                    if let Ok(records) = &result {
                        service.journal_pipeline_terminal_records(expiry_permit, records);
                    }
                    result
                }) {
                    Ok(records) => records,
                    Err(error) => {
                        error!("tx-pool pipeline expiry failed: {:?}", error);
                        break;
                    }
                };
                let dependency_permit = match service
                    .reserve_effects(TxPoolService::pipeline_terminal_effect_bytes(SLICE))
                    .await
                {
                    Ok(permit) => permit,
                    Err(error) => {
                        error!("tx-pool dependency effect reservation failed: {:?}", error);
                        break;
                    }
                };
                let failed = match service.pipeline.runtime.mutate(|coordinator| {
                    let result = coordinator.drain_dependency_failures(SLICE);
                    if let Ok(records) = &result {
                        service.journal_pipeline_terminal_records(dependency_permit, records);
                    }
                    result
                }) {
                    Ok(records) => records,
                    Err(error) => {
                        error!("tx-pool dependency maintenance failed: {:?}", error);
                        break;
                    }
                };
                let rechecked = match service
                    .pipeline
                    .runtime
                    .mutate(|coordinator| coordinator.drain_conflict_rechecks(SLICE))
                {
                    Ok(records) => records.len(),
                    Err(error) => {
                        error!("tx-pool conflict maintenance failed: {:?}", error);
                        break;
                    }
                };
                let saturated =
                    expired.len() == SLICE || failed.len() == SLICE || rechecked == SLICE;
                if rechecked != 0 {
                    service.drive_pipeline_commits().await;
                }
                if !saturated && !service.pipeline.runtime.maintenance_pending() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        }
        info!("TxPool pipeline maintenance worker exited");
    })
}

/// Spawn the verification manager monitor with panic-respawn protection,
/// mirroring [`spawn_resolver_monitor`]. The manager supervises its verify
/// workers internally, but nothing watched the manager task itself:
/// without this loop, a manager-level exit (panic or unexpected stop)
/// would silently stall the whole verification stage — the verify queue
/// would fill up and every new transaction would eventually be rejected as
/// `Reject::Full`, with no log at all. Returns the spawned task handle so
/// the shutdown path can quiesce it before persisting.
pub(crate) fn spawn_verify_mgr_monitor(
    handle: &Handle,
    service: TxPoolService,
    chunk_rx: watch::Receiver<ChunkCommand>,
    signal: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    handle.spawn(async move {
        let mut backoff = RespawnBackoff::new();
        loop {
            let mut verify_mgr = VerifyMgr::new(service.clone(), chunk_rx.clone(), signal.clone());
            let started = std::time::Instant::now();
            let outcome = AssertUnwindSafe(verify_mgr.run()).catch_unwind().await;
            match outcome {
                Ok(()) => {
                    if signal.is_cancelled() {
                        break;
                    }
                    error!("tx-pool verify manager stopped unexpectedly, respawning");
                }
                Err(payload) => {
                    error!(
                        "tx-pool verify manager panicked: {}; respawning",
                        crate::util::panic_payload_to_string(payload.as_ref())
                    );
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(backoff.delay_for(started.elapsed())) => {}
                _ = signal.cancelled() => break,
            }
        }
        info!("TxPool verify manager monitor exited");
    })
}

/// Spawn the ordered resolver monitor with panic-respawn protection.
/// Returns the spawned task handle so the shutdown path can quiesce it
/// before persisting.
pub(crate) fn spawn_resolver_monitor(
    handle: &Handle,
    service: TxPoolService,
    chunk_rx: watch::Receiver<ChunkCommand>,
    resolver_exit_signal: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    handle.spawn(async move {
        let mut backoff = RespawnBackoff::new();
        loop {
            let resolver = crate::resolve_mgr::OrderedResolver::new(
                service.clone(),
                chunk_rx.clone(),
                resolver_exit_signal.clone(),
            );
            let (exit_tx, mut exit_rx) = tokio::sync::mpsc::unbounded_channel();
            let handle = resolver.start(exit_tx);
            let started = std::time::Instant::now();

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
                    tokio::select! {
                        _ = tokio::time::sleep(backoff.delay_for(started.elapsed())) => {}
                        _ = resolver_exit_signal.cancelled() => break,
                    }
                }
            }
        }
        info!("TxPool ordered resolver monitor exited");
    })
}

/// Spawn the ordered, retained reorg handler.
pub(crate) fn spawn_reorg_handler(
    handle: &Handle,
    service: TxPoolService,
    reorg_receiver: mpsc::Receiver<Notify<ChainReorgArgs>>,
    signal_receiver: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    handle.spawn(async move {
        // Reorg deltas are ordered state transitions. Keep the received head
        // message until it succeeds; receiving the next delta first would let
        // a panic create a permanent tip/pool mismatch. Authoritative updates
        // are convergence-idempotent: repeated removals/status transitions are
        // no-ops, retained transactions re-add independently, fee-estimator
        // commits ignore an already-seen height, and template updates rebuild
        // from the current snapshot. User callbacks contain their own panics,
        // so external side effects cannot trap this retry loop.
        run_retained_receiver(
            "tx-pool reorg handler",
            reorg_receiver,
            &signal_receiver,
            |Notify {
                 arguments: (detached_blocks, attached_blocks, detached_proposal_id, snapshot),
             }| {
                let service = service.clone();
                async move {
                    service
                        .update_block_assembler_before_tx_pool_reorg(
                            detached_blocks.clone(),
                            Arc::clone(&snapshot),
                        )
                        .await;

                    service
                        .update_tx_pool_for_reorg(
                            detached_blocks,
                            attached_blocks,
                            detached_proposal_id,
                            snapshot,
                        )
                        .await;

                    service.update_block_assembler_after_tx_pool_reorg().await;
                }
            },
        )
        .await;
        if signal_receiver.is_cancelled() {
            info!("TxPool reorg process service received exit signal, exit now");
        } else {
            info!("TxPool reorg process service exited because its channel closed");
        }
    })
}

/// Spawn the deferred task worker with panic-respawn protection.
///
/// Recovery tx re-enqueue and verify cache updates run sequentially in a
/// single background task. The worker retains the authoritative runtime and
/// effect endpoints, but not `TxPoolService` (which owns the channel sender).
pub(crate) fn spawn_deferred_worker(
    handle: &Handle,
    runtime: Arc<crate::component::pipeline_runtime::PipelineRuntime>,
    txs_verify_cache: Arc<tokio::sync::RwLock<ckb_verification::cache::TxVerificationCache>>,
    deferred_receiver: mpsc::Receiver<DeferredTask>,
    cancel: CancellationToken,
    relay: crate::service::RelayState,
    recent_reject: Option<Arc<crate::component::recent_reject::RecentReject>>,
    epoch: Arc<crate::service::PipelineEpoch>,
) -> tokio::task::JoinHandle<()> {
    handle.spawn(async move {
        let mut deferred_rx = deferred_receiver;
        loop {
            // recv() is outside catch_unwind: the mpsc receiver is not
            // poisoned by panics in the message handler below.
            let task = tokio::select! {
                Some(task) = deferred_rx.recv() => task,
                _ = cancel.cancelled() => {
                    info!("deferred task worker received exit signal, draining remaining tasks");
                    // Best-effort drain: recovered txs lose their only handle
                    // when the channel is dropped, so coordinator-admit each
                    // with a single non-blocking attempt before exiting.
                    while let Ok(task) = deferred_rx.try_recv() {
                        if let DeferredTask::RecoverTxs(txs) = task {
                            crate::process::recover::enqueue_pipeline_recover_txs(
                                Arc::clone(&runtime),
                                txs,
                                &cancel,
                                &relay,
                                recent_reject.as_ref(),
                                &epoch,
                                false,
                            )
                            .await;
                        }
                    }
                    break;
                }
                else => break,
            };
            // Coalesce back-to-back recovery tasks: under RBF churn they
            // arrive in bursts, and each used to occupy its own bounded
            // retry window — merging keeps the deferred channel (and the
            // verify workers blocked on `send`) from backing up.
            let task = match task {
                DeferredTask::RecoverTxs(mut txs) => {
                    while let Ok(next) = deferred_rx.try_recv() {
                        match next {
                            DeferredTask::RecoverTxs(mut more) => txs.append(&mut more),
                            DeferredTask::CacheUpdate { wtx_hash, verified } => {
                                let mut guard = txs_verify_cache.write().await;
                                guard.put(wtx_hash, verified);
                            }
                        }
                    }
                    DeferredTask::RecoverTxs(txs)
                }
                other => other,
            };
            let runtime_handler = Arc::clone(&runtime);
            let runtime_retry = Arc::clone(&runtime);
            let txs_verify_cache = Arc::clone(&txs_verify_cache);
            let recover_txs_for_retry = match &task {
                DeferredTask::RecoverTxs(txs) => Some(txs.clone()),
                DeferredTask::CacheUpdate { .. } => None,
            };
            let cancel_handler = cancel.clone();
            let relay_handler = relay.clone();
            let recent_reject_handler = recent_reject.clone();
            let epoch_handler = Arc::clone(&epoch);
            let handler = async move {
                match task {
                    DeferredTask::RecoverTxs(txs) => {
                        crate::process::recover::enqueue_pipeline_recover_txs(
                            runtime_handler,
                            txs,
                            &cancel_handler,
                            &relay_handler,
                            recent_reject_handler.as_ref(),
                            &epoch_handler,
                            true,
                        )
                        .await;
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
                    error!("deferred task worker panicked: {}; continuing", message);
                    // If a RecoverTxs task panicked, retry it directly
                    // without going back through the channel. The retry is
                    // guarded too: a deterministic panic (same input, same
                    // failure) must not kill the worker — its death would
                    // close the deferred channel and silently drop every
                    // later recovery task.
                    if let Some(txs) = recover_txs_for_retry {
                        let retry = crate::worker::catch_job_panic(
                            crate::process::recover::enqueue_pipeline_recover_txs(
                                runtime_retry,
                                txs,
                                &cancel,
                                &relay,
                                recent_reject.as_ref(),
                                &epoch,
                                true,
                            ),
                        )
                        .await;
                        if let Err(message) = retry {
                            error!(
                                "deferred task worker retry panicked again: {}; dropping task",
                                message
                            );
                        }
                    }
                }
            }
        }
        info!("deferred task worker exited (channel closed)");
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respawn_backoff_progresses_caps_and_resets() {
        let mut backoff = RespawnBackoff::new();
        let crash = Duration::from_millis(5);

        // Consecutive crashes: 100ms, 200ms, 400ms, ...
        assert_eq!(backoff.delay_for(crash), Duration::from_millis(100));
        assert_eq!(backoff.delay_for(crash), Duration::from_millis(200));
        assert_eq!(backoff.delay_for(crash), Duration::from_millis(400));

        // Capped at 30s under a persistent failure.
        for _ in 0..20 {
            backoff.delay_for(crash);
        }
        assert_eq!(backoff.delay_for(crash), Duration::from_secs(30));

        // A healthy run (>= HEALTHY_RUN) resets the backoff to the base.
        assert_eq!(
            backoff.delay_for(Duration::from_secs(120)),
            Duration::from_millis(100)
        );
    }

    /// A received reorg delta must survive more than the historical single
    /// retry. The same head item remains selected until one attempt succeeds.
    #[tokio::test]
    async fn retained_message_retries_until_success() {
        let cancel = CancellationToken::new();
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_for_handler = Arc::clone(&attempts);
        let completed =
            retry_retained_message("test retained worker", 7usize, &cancel, move |item| {
                let attempts = Arc::clone(&attempts_for_handler);
                async move {
                    assert_eq!(item, 7);
                    if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 3 {
                        panic!("injected deterministic transition panic");
                    }
                }
            })
            .await;

        assert!(completed);
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 4);
    }

    /// Persistent failure must be cancel-aware while retaining the item; it
    /// must neither hot-spin nor report success/drop.
    #[tokio::test]
    async fn retained_message_cancellation_interrupts_backoff() {
        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let task = tokio::spawn(async move {
            retry_retained_message(
                "test retained worker",
                1usize,
                &cancel_for_task,
                |_| async { panic!("persistent injected panic") },
            )
            .await
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel.cancel();
        let completed = tokio::time::timeout(Duration::from_millis(200), task)
            .await
            .expect("cancellation must interrupt retry backoff")
            .expect("retry task joins");
        assert!(!completed, "cancelled retained work is not acknowledged");
    }

    /// Later chain deltas must never overtake a retained/panicking head delta.
    #[tokio::test]
    async fn retained_receiver_preserves_fifo_across_panics() {
        let (sender, receiver) = mpsc::channel(2);
        sender.send(1usize).await.unwrap();
        sender.send(2usize).await.unwrap();
        drop(sender);

        let cancel = CancellationToken::new();
        let first_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let processed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let attempts_for_handler = Arc::clone(&first_attempts);
        let processed_for_handler = Arc::clone(&processed);
        run_retained_receiver("test retained receiver", receiver, &cancel, move |item| {
            let attempts = Arc::clone(&attempts_for_handler);
            let processed = Arc::clone(&processed_for_handler);
            async move {
                if item == 1 && attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2 {
                    panic!("injected head transition panic");
                }
                processed.lock().unwrap().push(item);
            }
        })
        .await;

        assert_eq!(processed.lock().unwrap().as_slice(), &[1, 2]);
        assert_eq!(first_attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    /// Cancel during backoff sleep must exit immediately (bug #40):
    /// the `select!` wrapping the sleep must observe cancellation
    /// instead of waiting out the full backoff duration.
    #[tokio::test]
    async fn cancel_during_backoff_exits_immediately() {
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        // Spawn a task that simulates a worker in backoff sleep.
        let handle = tokio::spawn(async move {
            let mut backoff = RespawnBackoff::new();
            // Simulate a crash to get a non-trivial backoff delay.
            let _ = backoff.delay_for(Duration::from_millis(1));
            let _ = backoff.delay_for(Duration::from_millis(1));
            // Now the next delay would be 400ms.
            let delay = backoff.delay_for(Duration::from_millis(1));
            assert!(delay >= Duration::from_millis(400));

            let started = std::time::Instant::now();
            tokio::select! {
                _ = tokio::time::sleep(delay) => {
                    // Should not reach here if cancel fires first.
                    started.elapsed()
                }
                _ = cancel_clone.cancelled() => {
                    // Cancel fired: exit immediately.
                    started.elapsed()
                }
            }
        });

        // Give the task time to enter the select!.
        tokio::time::sleep(Duration::from_millis(50)).await;
        // Cancel: the task should exit well before the 400ms backoff.
        cancel.cancel();

        let elapsed = tokio::time::timeout(Duration::from_millis(200), handle)
            .await
            .expect("task must exit within 200ms of cancel, not wait out the backoff")
            .expect("task joins");

        assert!(
            elapsed < Duration::from_millis(150),
            "cancel must interrupt backoff sleep immediately, took {:?}",
            elapsed
        );
    }

    /// Cancel must drain pending `RecoverTxs` from the deferred channel
    /// before exiting (bug #41): recovered transactions lose their only
    /// handle when the channel is dropped, so they must be enqueued.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_drains_deferred_recover_txs() {
        use crate::service::DeferredTask;
        use ckb_types::core::TransactionBuilder;

        let runtime = Arc::new(crate::component::pipeline_runtime::PipelineRuntime::new(
            &ckb_app_config::TxPoolConfig::default(),
            &ckb_chain_spec::consensus::ConsensusBuilder::default().build(),
            CancellationToken::new(),
        ));
        let txs_verify_cache = Arc::new(tokio::sync::RwLock::new(
            ckb_verification::cache::init_cache(),
        ));
        let (deferred_tx, deferred_rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        let epoch = Arc::new(crate::service::PipelineEpoch::default());
        let (tx_relay_sender, _relay_rx) = ckb_channel::bounded(16);
        let (block_assembler_sender, _ba_rx) = tokio::sync::mpsc::channel(1);
        let relay = crate::service::RelayState {
            network: Arc::new(crate::network::DummyTxPoolNetwork),
            tx_relay_sender,
            block_assembler_sender,
            block_assembler_dirty: Arc::new(std::sync::atomic::AtomicU8::new(0)),
            callbacks: Arc::new(crate::callback::Callbacks::new()),
            effects: Arc::new(crate::service::effects::EffectQueue::new(16, 1_000_000).unwrap()),
            banned_peers: Default::default(),
        };

        // Enqueue two RecoverTxs tasks before cancelling.
        let tx1 = TransactionBuilder::default().build();
        let tx2 = TransactionBuilder::default()
            .input(ckb_types::packed::CellInput::new(
                ckb_types::packed::OutPoint::new(tx1.hash(), 0),
                0,
            ))
            .build();
        let id1 = tx1.proposal_short_id();
        let id2 = tx2.proposal_short_id();
        deferred_tx
            .send(DeferredTask::RecoverTxs(vec![
                crate::resolved_tx::ResolveJob::new_at(tx1, crate::tx_source::TxSource::Local, 0),
                crate::resolved_tx::ResolveJob::new_at(tx2, crate::tx_source::TxSource::Local, 0),
            ]))
            .await
            .unwrap();

        // Spawn the deferred worker, then cancel immediately.
        let ckb_handle = ckb_async_runtime::Handle::new(tokio::runtime::Handle::current(), None);
        let handle = spawn_deferred_worker(
            &ckb_handle,
            Arc::clone(&runtime),
            txs_verify_cache,
            deferred_rx,
            cancel.clone(),
            relay,
            None,
            epoch,
        );
        // Give the worker a moment to start and block on recv.
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();

        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("deferred worker must exit after cancel")
            .expect("task joins");

        // The RecoverTxs must have been drained into the coordinator.
        assert!(
            runtime.read(|coordinator| {
                coordinator.hash_by_short_id(&id1).is_some()
                    || coordinator.hash_by_short_id(&id2).is_some()
            }),
            "at least one recovered tx must be coordinator-owned on cancel"
        );
    }
}
