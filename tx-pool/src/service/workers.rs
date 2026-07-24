//! Background worker spawning for the tx-pool pipeline.
//!
//! Every long-running pipeline task is spawned here: the pre-check worker
//! pool, the verify-manager and ordered-resolver monitors (with
//! panic-respawn backoff), the reorg handler, and the verify-cache worker.
//! The service builder (`service::builder`) keeps only assembly, startup
//! and shutdown orchestration; worker lifecycle lives in this module.

use crate::service::{ChainReorgArgs, Notify, TxPoolService, VerifyCacheUpdate};
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

/// Retry two ordered phases independently. Once phase one has completed, a
/// deterministic phase-two failure must never replay it. Reorg uses this to
/// keep an assembler refresh retry from reapplying an already-committed
/// TxPool snapshot/membership transition after a concurrent clear.
async fn retry_retained_two_phase<T, F1, Fut1, F2, Fut2>(
    first_name: &'static str,
    second_name: &'static str,
    item: T,
    cancel: &CancellationToken,
    mut first: F1,
    mut second: F2,
) -> bool
where
    T: Clone,
    F1: FnMut(T) -> Fut1,
    Fut1: std::future::Future<Output = ()>,
    F2: FnMut(T) -> Fut2,
    Fut2: std::future::Future<Output = ()>,
{
    if !retry_retained_message(first_name, item.clone(), cancel, &mut first).await {
        return false;
    }
    retry_retained_message(second_name, item, cancel, &mut second).await
}

/// Consume an ordered channel without acknowledging the head item until its
/// handler succeeds. Later messages stay in the bounded channel, providing
/// backpressure and preserving transition order.
#[cfg(test)]
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
            Some(lease) => service.process_pipeline_raw_lease(lease).await,
            None => break,
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
                    Err(error) => service
                        .pipeline
                        .runtime
                        .fail_stop("tx-pool expiry effect reservation failed", &error),
                };
                let expired = service.pipeline.runtime.mutate_required(
                    "tx-pool pipeline expiry failed",
                    |coordinator| {
                        let result = coordinator.expire_due(now, SLICE);
                        if let Ok(records) = &result {
                            service.journal_pipeline_terminal_records(expiry_permit, records);
                        }
                        result
                    },
                );
                let dependency_permit = match service
                    .reserve_effects(TxPoolService::pipeline_terminal_effect_bytes(SLICE))
                    .await
                {
                    Ok(permit) => permit,
                    Err(error) => service
                        .pipeline
                        .runtime
                        .fail_stop("tx-pool dependency effect reservation failed", &error),
                };
                let failed = service.pipeline.runtime.mutate_required(
                    "tx-pool dependency maintenance failed",
                    |coordinator| {
                        let result = coordinator.drain_dependency_failures(SLICE);
                        if let Ok(records) = &result {
                            service.journal_pipeline_terminal_records(dependency_permit, records);
                        }
                        result
                    },
                );
                let rechecked = service
                    .pipeline
                    .runtime
                    .mutate_required("tx-pool conflict maintenance failed", |coordinator| {
                        coordinator.drain_conflict_rechecks(SLICE)
                    })
                    .len();
                let conflict_recovery = service.recover_conflict_cache_slice(SLICE).await;
                let saturated = expired.len() == SLICE
                    || failed.len() == SLICE
                    || rechecked == SLICE
                    || conflict_recovery.saturated;
                if rechecked != 0 {
                    service.drive_pipeline_commits().await;
                }
                if conflict_recovery.capacity_blocked {
                    // Avoid a hot loop against a globally full coordinator or
                    // outbox. The one-second maintenance tick retries the
                    // level-triggered cache queue.
                    break;
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
        let mut reorg_receiver = reorg_receiver;
        loop {
            let item = tokio::select! {
                item = reorg_receiver.recv() => item,
                _ = signal_receiver.cancelled() => None,
            };
            let Some(item) = item else {
                break;
            };
            let first_service = service.clone();
            let second_service = service.clone();
            let completed = retry_retained_two_phase(
                "tx-pool reorg transition",
                "block-assembler reorg refresh",
                item,
                &signal_receiver,
                move |Notify {
                          arguments:
                              (detached_blocks, attached_blocks, detached_proposal_id, snapshot),
                      }| {
                    let service = first_service.clone();
                    async move {
                        service
                            .update_tx_pool_for_reorg(
                                detached_blocks,
                                attached_blocks,
                                detached_proposal_id,
                                snapshot,
                            )
                            .await
                            .unwrap_or_else(|error| {
                                panic!("retryable tx-pool reorg transition failed: {error}")
                            });
                    }
                },
                move |Notify {
                          arguments: (detached_blocks, _, _, _),
                      }| {
                    let service = second_service.clone();
                    async move {
                        service
                            .refresh_block_assembler_after_tx_pool_reorg(detached_blocks)
                            .await
                            .unwrap_or_else(|error| {
                                panic!("retryable block-assembler reorg refresh failed: {error}")
                            });
                    }
                },
            )
            .await;
            if !completed {
                break;
            }
        }
        if signal_receiver.is_cancelled() {
            info!("TxPool reorg process service received exit signal, exit now");
        } else {
            info!("TxPool reorg process service exited because its channel closed");
        }
    })
}

/// Apply best-effort verification-cache writes without delaying commit.
/// Executable transactions never pass through this worker.
pub(crate) fn spawn_verify_cache_worker(
    handle: &Handle,
    txs_verify_cache: Arc<tokio::sync::RwLock<ckb_verification::cache::TxVerificationCache>>,
    receiver: mpsc::Receiver<VerifyCacheUpdate>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    handle.spawn(async move {
        let mut receiver = receiver;
        loop {
            let update = tokio::select! {
                Some(update) = receiver.recv() => update,
                _ = cancel.cancelled() => {
                    info!("verify-cache worker received exit signal, draining buffered updates");
                    while let Ok(update) = receiver.try_recv() {
                        let mut guard = txs_verify_cache.write().await;
                        guard.put(update.key, update.verified);
                    }
                    break;
                }
                else => break,
            };
            let mut guard = txs_verify_cache.write().await;
            guard.put(update.key, update.verified);
        }
        info!("verify-cache worker exited (channel closed)");
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

    #[tokio::test]
    async fn second_phase_retry_never_replays_completed_first_phase() {
        let cancel = CancellationToken::new();
        let first_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let second_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let first_counter = Arc::clone(&first_attempts);
        let second_counter = Arc::clone(&second_attempts);
        let completed = retry_retained_two_phase(
            "test authoritative phase",
            "test derived refresh phase",
            9usize,
            &cancel,
            move |item| {
                let attempts = Arc::clone(&first_counter);
                async move {
                    assert_eq!(item, 9);
                    attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            },
            move |item| {
                let attempts = Arc::clone(&second_counter);
                async move {
                    assert_eq!(item, 9);
                    if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        panic!("injected derived refresh failure");
                    }
                }
            },
        )
        .await;

        assert!(completed);
        assert_eq!(first_attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(second_attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
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
}
