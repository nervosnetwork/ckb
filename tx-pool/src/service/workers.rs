//! Background worker spawning for the tx-pool pipeline.
//!
//! Every long-running pipeline task is spawned here: the pre-check worker
//! pool, the verify-manager and ordered-resolver monitors (with
//! panic-respawn backoff), the reorg handler, and the verify-cache worker.
//! The service builder (`service::builder`) keeps only assembly, startup
//! and shutdown orchestration; worker lifecycle lives in this module.

use crate::component::pre_pool::WorkLane;
use crate::service::effects::{EffectClass, EffectJournalError};
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

/// The pre-check worker body. Ownership moves queued → active → resolved,
/// waiting, or terminal entirely inside the coordinator; there is no trailing
/// `finish` call that a stale worker could apply to a newer incarnation.
pub(crate) async fn run_pre_check_worker_loop(service: TxPoolService) {
    while let Some(lease) = service
        .pipeline
        .kernel
        .wait_resolve(crate::component::pre_pool::ResolveLane::Ingress)
        .await
    {
        service.process_pipeline_raw_lease(lease).await;
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

/// Consume the derived commit queue independently of the transition that made
/// a candidate eligible. Eligibility can change after verification, expiry,
/// dependency failure, administrative removal, or a failed commit; tying the
/// consumer to verify completion would therefore lose wake paths.
pub(crate) async fn run_pipeline_commit_worker(service: TxPoolService, cancel: CancellationToken) {
    let ready = service.pipeline.kernel.subscribe(
        WorkLane::Commit,
        crate::component::pre_pool::WorkCapability::Any,
    );
    let mut backoff = RespawnBackoff::new();
    loop {
        // Notify is only a hint. In particular, a driver panic consumes the
        // current permit while deliberately retaining the Ready owner. Read
        // the authoritative level before sleeping so that retained work is
        // retried after backoff without requiring an unrelated mutation.
        if service.pipeline.kernel.queue_is_empty(WorkLane::Commit) {
            tokio::select! {
                _ = ready.notified() => {}
                _ = cancel.cancelled() => break,
            }
        }
        if cancel.is_cancelled() {
            break;
        }
        let started = std::time::Instant::now();
        let outcome = AssertUnwindSafe(service.drive_pipeline_commits())
            .catch_unwind()
            .await;
        if let Err(payload) = outcome {
            if service.pipeline.kernel.is_failed() {
                break;
            }
            let message = crate::util::panic_payload_to_string(payload.as_ref());
            let delay = backoff.delay_for(started.elapsed());
            error!(
                "tx-pool commit driver panicked; retaining Ready owner and retrying in {:?}: {}",
                delay, message
            );
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = cancel.cancelled() => break,
            }
        } else {
            backoff = RespawnBackoff::new();
        }
    }
    info!("TxPool pipeline commit worker exited");
}

pub(crate) fn spawn_pipeline_commit_worker(
    handle: &Handle,
    service: TxPoolService,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    handle.spawn(run_pipeline_commit_worker(service, cancel))
}

/// Drain dependency cascades and remote expiry in bounded slices. Conflict
/// eligibility is derived synchronously inside each coordinator transition,
/// so maintenance is never part of the candidate liveness path. The
/// notification is level-triggered for graph work; a coarse timer is retained
/// only for wall-clock expiry. No slice can grow with the full
/// attacker-controlled graph while holding the coordinator mutex.
pub(crate) fn spawn_pipeline_maintenance_worker(
    handle: &Handle,
    service: TxPoolService,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    const SLICE: usize = 32;
    const EXPIRY_TICK: Duration = Duration::from_secs(1);

    handle.spawn(async move {
        let ready = service.pipeline.kernel.subscribe_maintenance();
        let mut expiry = tokio::time::interval(EXPIRY_TICK);
        expiry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = ready.notified() => {}
                _ = expiry.tick() => {}
                _ = cancel.cancelled() => break,
            }

            while !cancel.is_cancelled() {
                let now = ckb_systemtime::unix_time().as_secs();
                let preview = service
                    .pipeline
                    .kernel
                    .read(|kernel| kernel.due_terminal_records(now, SLICE, true));
                if let Some(batch) = service.pipeline_terminal_effects(&preview)
                    && service
                        .relay
                        .effects
                        .wait_capacity(batch.charge_bytes(), EffectClass::Trusted)
                        .await
                        .is_err()
                {
                    break;
                }
                // Ready has no transient commit state. Serializing expiry
                // with the commit driver is therefore the proof that a
                // read-only commit ticket remains owned until its paired
                // pool/kernel settlement finishes.
                let commit_guard = service.pipeline.kernel.try_lock_commit_driver();
                let expired = match service.pipeline.kernel.mutate(|coordinator| {
                    let records =
                        coordinator.due_terminal_records(now, SLICE, commit_guard.is_some());
                    let batch = service.pipeline_terminal_effects(&records);
                    service
                        .relay
                        .effects
                        .try_apply(batch, EffectClass::Trusted, || {
                            coordinator.expire_due(now, SLICE, commit_guard.is_some())
                        })
                }) {
                    Ok(Ok(records)) => records,
                    Ok(Err(error)) => {
                        ckb_logger::error!("tx-pool pipeline expiry failed: {error:?}");
                        break;
                    }
                    Err(EffectJournalError::Full) => {
                        drop(commit_guard);
                        continue;
                    }
                    Err(error) => {
                        ckb_logger::error!("tx-pool expiry journal unavailable: {error:?}");
                        break;
                    }
                };
                drop(commit_guard);
                let woke = service
                    .pipeline
                    .kernel
                    .mutate_required("tx-pool wait maintenance failed", |kernel| {
                        kernel.drain_wait_wakes(SLICE)
                    });
                let saturated = expired.len() == SLICE || woke == SLICE;
                if !saturated && !service.pipeline.kernel.maintenance_pending() {
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
#[path = "tests/workers.rs"]
mod tests;
