//! Background worker spawning for the tx-pool pipeline.
//!
//! Every long-running pipeline task is spawned here: the pre-check worker
//! pool, commit/maintenance, the reorg handler, and the verify-cache worker.
//! The service builder (`service::builder`) keeps only assembly, startup
//! and shutdown orchestration; worker lifecycle lives in this module.

use crate::component::pre_pool::WorkLane;
use crate::process::ReorgUpdateError;
use crate::service::effects::{EffectCapacityWaitError, EffectClass, EffectJournalError};
use crate::service::{ChainReorgArgs, Notify, TxPoolService, VerifyCacheUpdate};
use ckb_async_runtime::Handle;
use ckb_logger::info;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

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
            Err(error) => {
                service.fail_tx_pool_generation(
                    "pre-check checkout invariant failed",
                    &crate::process::TxPoolGenerationFault::PrePool(error.into_unexpected_fault()),
                );
                break;
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
    let ready = service.pipeline.kernel.subscribe_commit();
    loop {
        // Notify is only a hint; read the authoritative level before sleeping
        // so a permit consumed before this iteration cannot lose Ready work.
        // A driver panic is not retried here: supervision performs a
        // controlled shutdown and makes persistence ineligible.
        if service.pipeline.kernel.queue_is_empty(WorkLane::Commit) {
            tokio::select! {
                _ = ready.notified() => {}
                _ = cancel.cancelled() => break,
            }
        }
        if cancel.is_cancelled() {
            break;
        }
        if !service.drive_pipeline_commits().await {
            break;
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
                    .read(|kernel| kernel.due_terminal_records(now, SLICE));
                if let Some(batch) = service.pipeline_terminal_effects(&preview) {
                    // Every deadline is created by a Remote owner. Expiry is
                    // maintenance work, but its publication remains
                    // attacker-originated and must not borrow trusted
                    // headroom during a coordinated expiry wave.
                    match service
                        .relay
                        .effects
                        .wait_capacity(batch.charge_bytes(), EffectClass::Remote)
                        .await
                    {
                        Ok(()) => {}
                        Err(EffectCapacityWaitError::Closed) => break,
                        Err(error) => {
                            service.fail_tx_pool_generation(
                                "expiry effect capacity proof failed",
                                &crate::process::TxPoolGenerationFault::Effect(error.into()),
                            );
                            break;
                        }
                    }
                }
                // Commit ticket selection and Apply now share one kernel
                // critical section. Expiry can execute directly: whichever
                // transition acquires the authority first owns the entry, and
                // no read-only ticket survives between mutex acquisitions.
                let expired = if !preview.is_empty() {
                    let result = service.pipeline.kernel.mutate_authoritative(
                        |coordinator| -> Result<_, crate::component::pre_pool::PrePoolError> {
                            let Some(plan) = coordinator.plan_expiry(now, SLICE)? else {
                                return Ok(Ok(Vec::new()));
                            };
                            let batch = service.pipeline_terminal_effects(plan.records());
                            Ok(service
                                .relay
                                .effects
                                .try_apply(batch, EffectClass::Remote, || plan.apply()))
                        },
                    );
                    match result {
                        Ok(Ok(records)) => records,
                        Ok(Err(EffectJournalError::Full)) => continue,
                        Ok(Err(EffectJournalError::Closed)) => break,
                        Ok(Err(error)) => {
                            service.fail_tx_pool_generation(
                                "expiry effect journal invariant failed",
                                &crate::process::TxPoolGenerationFault::Effect(error),
                            );
                            break;
                        }
                        Err(error) => {
                            service.fail_tx_pool_generation(
                                "pipeline expiry invariant failed",
                                &crate::process::TxPoolGenerationFault::PrePool(
                                    error.into_unexpected_fault(),
                                ),
                            );
                            break;
                        }
                    }
                } else {
                    Vec::new()
                };
                let woke = match service
                    .pipeline
                    .kernel
                    .mutate_authoritative(|kernel| kernel.drain_wait_wakes(SLICE))
                {
                    Ok(woke) => woke,
                    Err(error) => {
                        service.fail_tx_pool_generation(
                            "wait maintenance invariant failed",
                            &crate::process::TxPoolGenerationFault::PrePool(
                                error.into_unexpected_fault(),
                            ),
                        );
                        break;
                    }
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
        // Reorg deltas are ordered authoritative transitions. Each TxPool/
        // kernel Apply executes exactly once. Template refresh is derived,
        // level-triggered state and must never retain this channel head: a
        // deterministic cellbase/template error cannot block later chain tips.
        let mut reorg_receiver = reorg_receiver;
        loop {
            let item = tokio::select! {
                item = reorg_receiver.recv() => item,
                _ = signal_receiver.cancelled() => None,
            };
            let Some(item) = item else {
                break;
            };
            let Notify {
                arguments: (detached_blocks, attached_blocks, detached_proposal_id, snapshot),
            } = item;
            // The authoritative phase is a prevalidated, total Apply. It must
            // never be replayed through a generic error loop: shutdown before
            // linearization is its only ordinary failure mode.
            let phase_two = match service
                .update_tx_pool_for_reorg(
                    detached_blocks,
                    attached_blocks,
                    detached_proposal_id,
                    snapshot,
                )
                .await
            {
                Ok(output) => output,
                Err(ReorgUpdateError::Effect(EffectJournalError::Closed)) => break,
                Err(error) => {
                    service.fail_tx_pool_generation(
                        "reorg authoritative update failed",
                        &crate::process::TxPoolGenerationFault::Reorg(error),
                    );
                    break;
                }
            };
            service
                .refresh_block_assembler_after_tx_pool_reorg(phase_two.0, phase_two.1)
                .await;
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
