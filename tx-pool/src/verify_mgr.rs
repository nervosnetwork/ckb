use crate::component::entry::TxEntry;
use crate::component::pipeline_coordinator::{
    CoordinatorError, CoordinatorFeeGate, CoordinatorSource, QueueKind, TerminalDisposition,
    VerifyWorkLease, WorkerCapability,
};
use crate::component::pipeline_runtime::PipelineVerifiedTx;
use crate::service::TxPoolService;
use crate::service::pipeline_ops::ParentWaitOutcome;
use crate::worker::{JobHandler, WorkerOutcome, WorkerRunner};
use ckb_logger::{error, info};
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

#[derive(Clone, Debug, PartialEq)]
enum WorkerRole {
    OnlySmallCycleTx,
    SubmitTimeFirst,
}

#[derive(Debug)]
enum WorkerExit {
    Stopped { role: WorkerRole },
    Panicked { role: WorkerRole, message: String },
}

#[derive(Clone)]
struct VerifyHandler {
    service: TxPoolService,
    role: WorkerRole,
    /// A clone of the command receiver used by `verify_and_submit_tx` to check
    /// for pause/cancel while verifying. `WorkerRunner` holds another clone for
    /// its own select loop; sharing the same watch channel is cheap and correct.
    command_rx: watch::Receiver<ChunkCommand>,
}

impl TxPoolService {
    async fn settle_pipeline_verify_failure(
        &self,
        lease: &VerifyWorkLease<crate::resolved_tx::ResolvedTx>,
        disposition: TerminalDisposition,
        mut reject: crate::error::Reject,
        internal: bool,
    ) {
        if !internal
            && let crate::error::Reject::Resolve(ckb_types::core::error::OutPointError::Unknown(
                outpoint,
            )) = &reject
        {
            let parents = HashSet::from([outpoint.tx_hash()]);
            let permit = self
                .reserve_required_effects(
                    Self::unknown_parents_effect_bytes(parents.len()),
                    "verify parent-wait effect reservation failed",
                )
                .await;
            match self.settle_verify_parent_wait(lease, parents, permit).await {
                Some(ParentWaitOutcome::Parked) => return,
                Some(ParentWaitOutcome::Requeued) => return,
                Some(ParentWaitOutcome::Unavailable) => {}
                Some(ParentWaitOutcome::Rejected(wait_reject)) => reject = wait_reject,
                None => return,
            }
        }
        let public_reject = (!internal).then_some(reject);
        self.settle_pipeline_terminal(
            public_reject,
            "verify terminal effect reservation failed",
            "current verify lease could not terminalize",
            |coordinator| coordinator.terminalize_verification(lease, disposition),
        )
        .await;
    }

    pub(crate) async fn process_pipeline_verify_lease(
        &self,
        lease: VerifyWorkLease<crate::resolved_tx::ResolvedTx>,
        command_rx: &mut watch::Receiver<ChunkCommand>,
    ) {
        let authority = self.pipeline.runtime.read(|coordinator| {
            coordinator.view(&lease.hash).and_then(|view| {
                coordinator
                    .raw_by_hash(&lease.hash)
                    .map(|raw| (view.source, raw))
            })
        });
        let Some((current_source, raw)) = authority else {
            return;
        };
        let source = self
            .pipeline
            .runtime
            .require_authoritative_source(&raw, current_source);
        let epoch = raw.admitted_epoch;
        if !self.is_pipeline_epoch_current(epoch) || self.is_recently_banned(source) {
            self.settle_pipeline_verify_failure(
                &lease,
                TerminalDisposition::Internal,
                crate::error::Reject::Internal(
                    "pipeline verification invalidated before execution".to_string(),
                ),
                true,
            )
            .await;
            return;
        }

        // Queued resolved work is deliberately snapshot-free. Pin a database
        // snapshot only for this bounded active verification slot, and never
        // verify a resolution assembled at a different tip. Re-resolution
        // releases the old payload and produces a fresh, internally
        // consistent resolved bundle.
        let verification_snapshot = self.pool.tx_pool.read().await.cloned_snapshot();
        if verification_snapshot.tip_hash() != lease.payload.pre_resolve_tip {
            self.pipeline.runtime.mutate_lease(
                "stale resolved verification could not return to resolve",
                |coordinator| coordinator.verification_retry_resolution(&lease, HashSet::new()),
            );
            return;
        }

        let mut resolved = (*lease.payload).clone();
        resolved.source = source;
        let outcome = crate::worker::catch_job_panic(async {
            let first = self
                .verify_pipeline_resolved(
                    resolved.clone(),
                    Arc::clone(&verification_snapshot),
                    Some(&mut *command_rx),
                )
                .await;

            // A remote admission is verified against its declared cycle cap.
            // If the same full hash is promoted to Local/Proposal while that
            // work is active, the remote declaration is no longer
            // authoritative. Re-read ownership only on failure and retry once
            // with the trusted consensus cap. Without this edge, a bad remote
            // declaration can make a concurrent local/proposal promotion fail
            // even though the promoted transaction is valid.
            let mut verified = match first {
                Ok(verified) => verified,
                Err(first_reject) => {
                    let promoted = if source.peer().is_some() {
                        let authority = self.pipeline.runtime.read(|coordinator| {
                            coordinator.view(&lease.hash).and_then(|view| {
                                if matches!(view.source, CoordinatorSource::Remote(_)) {
                                    None
                                } else {
                                    coordinator
                                        .raw_by_hash(&lease.hash)
                                        .map(|raw| (view.source, raw))
                                }
                            })
                        });
                        authority.map(|(promoted_source, raw)| {
                            self.pipeline
                                .runtime
                                .require_authoritative_source(&raw, promoted_source)
                        })
                    } else {
                        None
                    };
                    match promoted {
                        Some(trusted_source) => {
                            resolved.source = trusted_source;
                            match self
                                .verify_pipeline_resolved(
                                    resolved.clone(),
                                    Arc::clone(&verification_snapshot),
                                    Some(&mut *command_rx),
                                )
                                .await
                            {
                                Ok(verified) => verified,
                                Err(reject) => {
                                    self.settle_pipeline_verify_failure(
                                        &lease,
                                        TerminalDisposition::Rejected,
                                        reject,
                                        false,
                                    )
                                    .await;
                                    return;
                                }
                            }
                        }
                        None => {
                            self.settle_pipeline_verify_failure(
                                &lease,
                                TerminalDisposition::Rejected,
                                first_reject,
                                false,
                            )
                            .await;
                            return;
                        }
                    }
                }
            };

            // Source promotion is allowed while verification is active. Bind
            // the completed payload to the current coordinator source rather
            // than the worker's checkout snapshot.
            let final_authority = self.pipeline.runtime.read(|coordinator| {
                coordinator.view(&lease.hash).and_then(|view| {
                    coordinator
                        .raw_by_hash(&lease.hash)
                        .map(|raw| (view.source, raw))
                })
            });
            let Some((current_source, raw)) = final_authority else {
                return;
            };
            let final_source = self
                .pipeline
                .runtime
                .require_authoritative_source(&raw, current_source);
            verified.candidate.source = final_source;

            let entry = TxEntry::new_with_resident_size(
                Arc::clone(&verified.candidate.rtx),
                verified.completed.cycles,
                verified.candidate.fee,
                verified.candidate.tx_size,
                verified.candidate.resident_size,
            );
            let rbf_precheck = {
                let pool = self.pool.tx_pool.read().await;
                if let Some(outpoint) = pool.pool_map.find_conflict_outpoint(entry.transaction()) {
                    if pool.enable_rbf() {
                        pool.check_rbf(&pool.cloned_snapshot(), &entry).map(|_| ())
                    } else {
                        Err(crate::error::Reject::Resolve(
                            ckb_types::core::error::OutPointError::Dead(outpoint),
                        ))
                    }
                } else {
                    Ok(())
                }
            };
            if let Err(reject) = rbf_precheck {
                self.settle_pipeline_verify_failure(
                    &lease,
                    TerminalDisposition::Rejected,
                    reject,
                    false,
                )
                .await;
                return;
            }

            let inputs: HashSet<_> = verified.candidate.tx.input_pts_iter().collect();
            let candidate = match CoordinatorFeeGate::new(0, 0).validate(
                lease.hash.clone(),
                inputs,
                verified.candidate.fee.as_u64(),
                verified.candidate.tx_size,
            ) {
                Ok(candidate) => candidate,
                Err(error) => {
                    let reject = self.pipeline.runtime.reject_or_fail(
                        "verified candidate fee gate violated coordinator invariants",
                        error,
                    );
                    self.settle_pipeline_verify_failure(
                        &lease,
                        TerminalDisposition::Rejected,
                        reject,
                        false,
                    )
                    .await;
                    return;
                }
            };
            let charge_bytes = match verified
                .candidate
                .resident_size
                .checked_add(std::mem::size_of::<PipelineVerifiedTx>())
                .ok_or(CoordinatorError::ResidencyChargeOverflow)
            {
                Ok(charge) => charge,
                Err(error) => {
                    let reject = self.pipeline.runtime.reject_or_fail(
                        "verified payload charge violated coordinator invariants",
                        error,
                    );
                    self.settle_pipeline_verify_failure(
                        &lease,
                        TerminalDisposition::Rejected,
                        reject,
                        false,
                    )
                    .await;
                    return;
                }
            };

            let permit = self
                .reserve_required_effects(
                    Self::pipeline_terminal_effect_bytes(
                        crate::constants::MAX_POOL_MUTATION_CANDIDATES.saturating_add(1),
                    ),
                    "verify completion effect reservation failed",
                )
                .await;
            match self.pipeline.runtime.mutate(|coordinator| {
                let result = coordinator.complete_verification_candidate(
                    &lease,
                    verified,
                    charge_bytes,
                    candidate,
                );
                if let Ok((_version, terminal)) = &result {
                    self.journal_pipeline_terminal_records(permit, terminal);
                }
                result
            }) {
                // Eagerly drain the candidate produced by this verify task.
                // The dedicated commit consumer is still the level-triggered
                // liveness path for eligibility created by every other
                // transition; both paths share the same serial driver.
                Ok((_version, _terminal)) => {
                    self.drive_pipeline_commits().await;
                }
                Err(error) if error.is_stale_lease() => {}
                Err(error) => {
                    let reject = self.pipeline.runtime.reject_or_fail(
                        "verification completion violated coordinator invariants",
                        error,
                    );
                    let internal = matches!(reject, crate::error::Reject::Full(_));
                    self.settle_pipeline_verify_failure(
                        &lease,
                        TerminalDisposition::Rejected,
                        reject,
                        internal,
                    )
                    .await;
                }
            }
        })
        .await;

        if let Err(message) = outcome {
            error!(
                "tx-pool verify worker panicked on {}: {}",
                lease.hash, message
            );
            // An authoritative pool/coordinator boundary panic has already
            // latched service-wide fail-closed state. Do not attempt to
            // reserve effects or settle the now-obsolete verify lease during
            // shutdown; that only creates a second panic and cannot restore a
            // transaction whose pool mutation may be partial.
            if self.pipeline.runtime.is_failed() {
                return;
            }
            self.settle_pipeline_verify_failure(
                &lease,
                TerminalDisposition::Internal,
                crate::error::Reject::Internal(message),
                true,
            )
            .await;
        }
    }
}

impl JobHandler for VerifyHandler {
    type Job = VerifyWorkLease<crate::resolved_tx::ResolvedTx>;
    type Exit = WorkerExit;

    fn worker_name(&self) -> &'static str {
        "verify worker"
    }

    async fn is_queue_empty(&self) -> bool {
        self.service
            .pipeline
            .runtime
            .queue_is_empty(QueueKind::Verify)
    }

    async fn queue_ready(&self) -> Arc<tokio::sync::Notify> {
        let capability = if self.role == WorkerRole::OnlySmallCycleTx {
            WorkerCapability::SmallCycleOnly
        } else {
            WorkerCapability::Any
        };
        self.service
            .pipeline
            .runtime
            .subscribe(QueueKind::Verify, capability)
    }

    async fn pop_one(&mut self) -> Option<VerifyWorkLease<crate::resolved_tx::ResolvedTx>> {
        let capability = if self.role == WorkerRole::OnlySmallCycleTx {
            WorkerCapability::SmallCycleOnly
        } else {
            WorkerCapability::Any
        };
        self.service
            .pipeline
            .runtime
            .mutate_required("verify checkout failed", |coordinator| {
                coordinator.checkout_verify(capability)
            })
    }

    async fn process_one(&mut self, work: VerifyWorkLease<crate::resolved_tx::ResolvedTx>) {
        self.service
            .process_pipeline_verify_lease(work, &mut self.command_rx)
            .await;
    }

    fn make_exit(&self, outcome: WorkerOutcome) -> WorkerExit {
        match outcome {
            WorkerOutcome::Stopped => WorkerExit::Stopped {
                role: self.role.clone(),
            },
            WorkerOutcome::Panicked(message) => WorkerExit::Panicked {
                role: self.role.clone(),
                message,
            },
        }
    }
}

pub(crate) struct VerifyMgr {
    workers: Vec<WorkerRunner<VerifyHandler>>,
    join_handles: Option<Vec<Option<JoinHandle<()>>>>,
    /// Per-generation cancellation token (child of the service shutdown
    /// signal). Dropping the manager — e.g. after a manager-level panic,
    /// before the monitor respawns a new one — cancels it, so the previous
    /// generation's workers shut down instead of doubling up with the
    /// respawned generation. Workers still see the service shutdown
    /// through the parent token.
    generation_signal: CancellationToken,
}

impl Drop for VerifyMgr {
    fn drop(&mut self) {
        self.generation_signal.cancel();
    }
}

impl VerifyMgr {
    pub fn new(
        service: TxPoolService,
        command_rx: watch::Receiver<ChunkCommand>,
        signal_exit: CancellationToken,
    ) -> Self {
        let generation_signal = signal_exit.child_token();
        // Clamp like the other spawn sites (pre-check pool, dispatcher
        // semaphore): a zero config would silently stall verification —
        // no workers to drain the queue and no log explaining why.
        let worker_num = service.pool.tx_pool_config.max_tx_verify_workers.max(1);
        let workers: Vec<_> = (0..worker_num)
            .map({
                let generation_signal = generation_signal.clone();
                move |idx| {
                    let role = if idx == 0 && worker_num > 1 {
                        WorkerRole::OnlySmallCycleTx
                    } else {
                        WorkerRole::SubmitTimeFirst
                    };
                    let handler = VerifyHandler {
                        service: service.clone(),
                        role,
                        command_rx: command_rx.clone(),
                    };
                    WorkerRunner::new(handler, command_rx.clone(), generation_signal.clone())
                }
            })
            .collect();
        Self {
            workers,
            join_handles: None,
            generation_signal,
        }
    }

    fn spawn_worker(
        &mut self,
        worker_id: usize,
        exit_tx: mpsc::UnboundedSender<(usize, WorkerExit)>,
    ) {
        let Some(worker) = self.workers.get(worker_id).cloned() else {
            error!("cannot respawn missing tx-pool verify worker {}", worker_id);
            return;
        };
        let handle = worker.start(worker_id, exit_tx);
        if let Some(handles) = self.join_handles.as_mut()
            && let Some(handle_slot) = handles.get_mut(worker_id)
        {
            handle_slot.replace(handle);
        } else {
            error!(
                "cannot store handle for tx-pool verify worker {}",
                worker_id
            );
        }
    }

    async fn join_worker(&mut self, worker_id: usize) {
        let handle = self
            .join_handles
            .as_mut()
            .and_then(|handles| handles.get_mut(worker_id))
            .and_then(Option::take);

        if let Some(handle) = handle
            && let Err(err) = handle.await
        {
            error!(
                "tx-pool verify worker {} join failed after exit notification: {}",
                worker_id, err
            );
        }
    }

    async fn start_loop(&mut self) {
        let (worker_exit_tx, mut worker_exit_rx) = mpsc::unbounded_channel();
        let mut join_handles = Vec::new();
        for (worker_id, w) in self.workers.iter_mut().enumerate() {
            let h = w.clone().start(worker_id, worker_exit_tx.clone());
            join_handles.push(Some(h));
        }
        self.join_handles.replace(join_handles);
        loop {
            tokio::select! {
                _ = self.generation_signal.cancelled() => {
                    info!("TxPool chunk_command service received exit signal, exit now");
                    // Workers will exit via their own CancellationToken;
                    // no need to broadcast Stop through per-worker channels.
                    break;
                },
                Some((worker_id, exit)) = worker_exit_rx.recv() => {
                    self.join_worker(worker_id).await;
                    if self.generation_signal.is_cancelled() {
                        continue;
                    }
                    match exit {
                        WorkerExit::Stopped { role } => {
                            error!(
                                "tx-pool verify worker {} ({:?}) stopped unexpectedly, respawning",
                                worker_id, role
                            );
                        }
                        WorkerExit::Panicked { role, message } => {
                            error!(
                                "tx-pool verify worker {} ({:?}) panicked: {}; respawning",
                                worker_id, role, message
                            );
                        }
                    }
                    self.spawn_worker(worker_id, worker_exit_tx.clone());
                }
            }
        }
        if let Some(jh) = self.join_handles.take() {
            for h in jh.into_iter().flatten() {
                if let Err(err) = h.await {
                    error!("tx-pool verify worker join failed: {}", err);
                }
            }
        }
        info!("TxPool verify_mgr service exited");
    }

    pub async fn run(&mut self) {
        self.start_loop().await;
    }
}
