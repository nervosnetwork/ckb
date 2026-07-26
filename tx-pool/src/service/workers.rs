//! Background worker spawning for the tx-pool pipeline.
//!
//! Every long-running pipeline task is spawned here: the pre-check worker
//! pool, commit/maintenance, the reorg handler, and the verify-cache worker.
//! The service builder (`service::builder`) keeps only assembly, startup
//! and shutdown orchestration; worker lifecycle lives in this module.

use crate::component::pre_pool::WorkLane;
use crate::service::effects::{EffectClass, EffectJournalError};
use crate::service::{ChainReorgArgs, Notify, TxPoolService, VerifyCacheUpdate};
use crate::worker::RespawnBackoff;
use ckb_async_runtime::Handle;
use ckb_logger::{error, info};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Retain one ordered state-transition message across explicit retryable
/// errors.  Panics are invariant failures, not a transport for retry control.
async fn retry_retained_message<T, E, F, Fut>(
    worker_name: &'static str,
    item: T,
    cancel: &CancellationToken,
    mut handler: F,
) -> bool
where
    T: Clone,
    F: FnMut(T) -> Fut,
    Fut: std::future::Future<Output = Result<(), E>>,
    E: std::fmt::Debug,
{
    let mut backoff = RespawnBackoff::new();
    loop {
        if cancel.is_cancelled() {
            return false;
        }
        let started = std::time::Instant::now();
        match handler(item.clone()).await {
            Ok(()) => return true,
            Err(error) => {
                let delay = backoff.delay_for(started.elapsed());
                error!(
                    "{} failed; retaining head message and retrying in {:?}: {:?}",
                    worker_name, delay, error
                );
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = cancel.cancelled() => return false,
                }
            }
        }
    }
}

/// Output-producing counterpart used only for the first reorg phase. The
/// successful output is the bounded phase-two authority token; the original
/// transaction-bearing input is dropped when this function returns.
async fn retry_retained_output<T, U, E, F, Fut>(
    worker_name: &'static str,
    item: T,
    cancel: &CancellationToken,
    mut handler: F,
) -> Option<U>
where
    T: Clone,
    F: FnMut(T) -> Fut,
    Fut: std::future::Future<Output = Result<U, E>>,
    E: std::fmt::Debug,
{
    let mut backoff = RespawnBackoff::new();
    loop {
        if cancel.is_cancelled() {
            return None;
        }
        let started = std::time::Instant::now();
        match handler(item.clone()).await {
            Ok(output) => return Some(output),
            Err(error) => {
                let delay = backoff.delay_for(started.elapsed());
                error!(
                    "{} failed; retaining head message and retrying in {:?}: {:?}",
                    worker_name, delay, error
                );
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = cancel.cancelled() => return None,
                }
            }
        }
    }
}

/// Retry two ordered phases independently. Once phase one has completed, a
/// deterministic phase-two failure must never replay it. Reorg uses this to
/// keep an assembler refresh retry from reapplying an already-committed
/// TxPool snapshot/membership transition after a concurrent clear.
async fn retry_retained_two_phase<T, U, E1, E2, F1, Fut1, F2, Fut2>(
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
    Fut1: std::future::Future<Output = Result<U, E1>>,
    E1: std::fmt::Debug,
    U: Clone,
    F2: FnMut(U) -> Fut2,
    Fut2: std::future::Future<Output = Result<(), E2>>,
    E2: std::fmt::Debug,
{
    let Some(phase_two) = retry_retained_output(first_name, item, cancel, &mut first).await else {
        return false;
    };
    retry_retained_message(second_name, phase_two, cancel, &mut second).await
}

/// The pre-check worker body. Ownership moves queued → active → resolved,
/// waiting, or terminal entirely inside the coordinator; there is no trailing
/// `finish` call that a stale worker could apply to a newer incarnation.
pub(crate) async fn run_pre_check_worker_loop(service: TxPoolService) {
    loop {
        match service
            .pipeline
            .kernel
            .wait_resolve(crate::component::pre_pool::ResolveLane::Ingress)
            .await
        {
            Ok(Some(lease)) => service.process_pipeline_raw_lease(lease).await,
            Ok(None) => break,
            Err(error) => panic!("pre-check checkout invariant failed: {error:?}"),
        }
    }
}

/// Spawn a pool of pre-check workers that pop jobs from the queue and
/// classify them into the pipeline.  Returns the spawned task handles so
/// the shutdown path can quiesce them before persisting.
pub(crate) fn spawn_pre_check_workers(
    handle: &Handle,
    service: TxPoolService,
    _pre_check_cancel: CancellationToken,
    count: usize,
) -> Vec<tokio::task::JoinHandle<()>> {
    let mut handles = Vec::with_capacity(count);
    for _ in 0..count {
        let svc = service.clone();
        let handle = handle.spawn(run_pre_check_worker_loop(svc));
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
        service.drive_pipeline_commits().await;
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
                let expired = match service.pipeline.kernel.mutate_authoritative(|coordinator| {
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
                    Ok(Ok(records)) => {
                        drop(commit_guard);
                        records
                    }
                    Ok(Err(error)) => {
                        drop(commit_guard);
                        panic!("pipeline expiry invariant failed: {error:?}")
                    }
                    Err(EffectJournalError::Full) => {
                        drop(commit_guard);
                        continue;
                    }
                    Err(error) => {
                        drop(commit_guard);
                        ckb_logger::error!("tx-pool expiry journal unavailable: {error:?}");
                        break;
                    }
                };
                let woke = match service
                    .pipeline
                    .kernel
                    .transition("wait maintenance mutation panicked", |kernel| {
                        kernel.drain_wait_wakes(SLICE)
                    }) {
                    Ok(woke) => woke,
                    Err(error) => panic!("wait maintenance invariant failed: {error:?}"),
                };
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
                    }
                },
                move |(candidate_uncles, snapshot)| {
                    let service = second_service.clone();
                    async move {
                        service
                            .refresh_block_assembler_after_tx_pool_reorg(candidate_uncles, snapshot)
                            .await
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
