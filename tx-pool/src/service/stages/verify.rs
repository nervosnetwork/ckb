use super::runner::{JobHandler, WorkerRunner};
use crate::component::entry::TxEntry;
use crate::component::pre_pool::PipelineVerifiedTx;
use crate::component::pre_pool::{
    DependencyKey, PrePoolError, PrePoolFault, PrePoolPublicError, PrePoolRejection, PrePoolSource,
    VerifyLease, WorkCapability, WorkLane,
};
use crate::service::TxPoolService;
use crate::service::pipeline_ops::ParentWaitOutcome;
use ckb_async_runtime::Handle;
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use std::sync::Arc;
use tokio::sync::watch;

#[derive(Clone)]
struct VerifyHandler {
    service: TxPoolService,
    capability: WorkCapability,
    /// A clone of the command receiver used by `verify_and_submit_tx` to check
    /// for pause/cancel while verifying. `WorkerRunner` holds another clone for
    /// its own select loop; sharing the same watch channel is cheap and correct.
    command_rx: watch::Receiver<ChunkCommand>,
}

/// Why an active verification owner is leaving the pipeline.
///
/// A policy/capacity rejection is observable by the submitter; administrative
/// invalidation is not. Keeping those outcomes in distinct variants prevents a
/// normal `Reject::Full` from being silently reclassified as worker cleanup.
enum VerifyFailure {
    Rejected(crate::error::Reject),
    Invalidated,
}

impl TxPoolService {
    async fn settle_pipeline_verify_failure(&self, lease: &VerifyLease, failure: VerifyFailure) {
        let mut reject = match failure {
            VerifyFailure::Rejected(reject) => Some(reject),
            VerifyFailure::Invalidated => None,
        };
        if let Some(crate::error::Reject::Resolve(
            ckb_types::core::error::OutPointError::Unknown(outpoint),
        )) = &reject
        {
            let dependencies = std::collections::BTreeSet::from([DependencyKey::Cell(
                crate::util::compact_packed(outpoint),
            )]);
            match self.settle_verify_parent_wait(lease, dependencies).await {
                Some(ParentWaitOutcome::Parked) => return,
                Some(ParentWaitOutcome::Requeued) => return,
                Some(ParentWaitOutcome::Unavailable) => {}
                Some(ParentWaitOutcome::Rejected(wait_reject)) => reject = Some(wait_reject),
                None => return,
            }
        }
        self.settle_pipeline_terminal(
            &lease.hash,
            reject,
            "current verify lease could not terminalize",
            |kernel, disposition| {
                if matches!(
                    disposition,
                    crate::component::pre_pool::ConflictDisposition::Retain
                ) {
                    kernel.park_conflict_or_terminalize(
                        &lease.hash,
                        lease.version,
                        crate::component::pre_pool::PrePoolLocation::VerifyLeased,
                    )
                } else {
                    kernel.terminalize_verify(lease)
                }
            },
        )
        .await;
    }

    pub(crate) async fn process_pipeline_verify_lease(
        &self,
        lease: VerifyLease,
        command_rx: &mut watch::Receiver<ChunkCommand>,
    ) {
        let authority = self.pipeline.kernel.read(|kernel| {
            kernel
                .source_by_hash(&lease.hash)
                .zip(kernel.raw_by_hash(&lease.hash))
        });
        let Some((current_source, raw)) = authority else {
            return;
        };
        let source = raw.authoritative_source(current_source);
        let epoch = raw.admitted_epoch;
        if !self.is_pipeline_epoch_current(epoch) || self.is_recently_banned(source) {
            self.settle_pipeline_verify_failure(&lease, VerifyFailure::Invalidated)
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
            match self.pipeline.kernel.mutate_authoritative(|kernel| {
                kernel.verification_retry_resolution(&lease, std::collections::BTreeSet::new())
            }) {
                Ok(_) => {}
                Err(error) if error.is_stale_lease() => {}
                Err(error) => self.fail_tx_pool_generation(
                    "stale verify requeue invariant failed",
                    &crate::process::TxPoolGenerationFault::PrePool(error.into_unexpected_fault()),
                ),
            }
            return;
        }

        let mut resolved = (*lease.payload).clone();
        resolved.source = source;
        let first = self
            .verify_pipeline_resolved(
                resolved.clone(),
                Arc::clone(&verification_snapshot),
                Some(&mut *command_rx),
            )
            .await;

        // A remote admission is verified against its declared cycle cap.
        // If the same full hash is promoted to Local/Proposal while that
        // work is active, the remote declaration is no longer authoritative.
        // Re-read ownership only on failure and retry once with the trusted
        // consensus cap. Without this edge, a bad remote declaration can make
        // a concurrent local/proposal promotion fail even though the promoted
        // transaction is valid.
        let mut verified = match first {
            Ok(verified) => verified,
            Err(first_reject) => {
                let promoted = if source.peer().is_some() {
                    let authority = self.pipeline.kernel.read(|kernel| {
                        kernel
                            .source_by_hash(&lease.hash)
                            .filter(|source| !matches!(source, PrePoolSource::Remote(_)))
                            .zip(kernel.raw_by_hash(&lease.hash))
                    });
                    authority
                        .map(|(promoted_source, raw)| raw.authoritative_source(promoted_source))
                } else {
                    None
                };
                match promoted {
                    Some(trusted_source) => {
                        resolved.source = trusted_source;
                        let promoted_result = self
                            .verify_pipeline_resolved(
                                resolved.clone(),
                                Arc::clone(&verification_snapshot),
                                Some(&mut *command_rx),
                            )
                            .await;
                        match promoted_result {
                            Ok(verified) => verified,
                            Err(reject) => {
                                self.settle_pipeline_verify_failure(
                                    &lease,
                                    VerifyFailure::Rejected(reject),
                                )
                                .await;
                                return;
                            }
                        }
                    }
                    None => {
                        self.settle_pipeline_verify_failure(
                            &lease,
                            VerifyFailure::Rejected(first_reject),
                        )
                        .await;
                        return;
                    }
                }
            }
        };

        // Source promotion is allowed while verification is active. Bind
        // the completed payload to the current kernel source rather
        // than the worker's checkout snapshot.
        let final_authority = self.pipeline.kernel.read(|kernel| {
            kernel
                .source_by_hash(&lease.hash)
                .zip(kernel.raw_by_hash(&lease.hash))
        });
        let Some((current_source, raw)) = final_authority else {
            return;
        };
        let final_source = raw.authoritative_source(current_source);
        verified.candidate.source = final_source;

        let entry = TxEntry::new_with_resident_size(
            Arc::clone(&verified.candidate.rtx),
            verified.completed.cycles,
            verified.candidate.fee,
            verified.candidate.tx_size,
            verified.candidate.resident_size,
        );
        let rbf_precheck =
            {
                let pool = self.pool.tx_pool.read().await;
                if let Some(outpoint) = pool.pool_map.find_conflict_outpoint(entry.transaction()) {
                    if pool.enable_rbf() {
                        pool.check_rbf(&pool.cloned_snapshot(), &entry).map(|_| ())
                    } else {
                        Err(crate::error::Reject::Resolve(
                            ckb_types::core::error::OutPointError::Dead(outpoint),
                        )
                        .into())
                    }
                } else {
                    Ok(())
                }
            };
        match rbf_precheck {
            Ok(()) => {}
            Err(crate::component::pool_map::PoolMutationPlanningError::Policy(reject)) => {
                self.settle_pipeline_verify_failure(&lease, VerifyFailure::Rejected(reject))
                    .await;
                return;
            }
            Err(crate::component::pool_map::PoolMutationPlanningError::Fault(error)) => {
                self.fail_tx_pool_generation(
                    "RBF verification precheck projection failed",
                    &crate::process::TxPoolGenerationFault::AcceptedPool(error),
                );
                return;
            }
        }

        let charge_bytes = match verified
            .candidate
            .resident_size
            .checked_add(std::mem::size_of::<PipelineVerifiedTx>())
            .ok_or(PrePoolPublicError::Rejected(
                PrePoolRejection::ResidencyChargeOverflow,
            )) {
            Ok(charge) => charge,
            Err(error) => {
                let reject = crate::component::pre_pool::pre_pool_reject(error);
                self.settle_pipeline_verify_failure(&lease, VerifyFailure::Rejected(reject))
                    .await;
                return;
            }
        };

        match self
            .pipeline
            .kernel
            .mutate_authoritative(|kernel| kernel.complete_verify(&lease, verified, charge_bytes))
        {
            // Eagerly drain the candidate produced by this verify task.
            // The dedicated commit consumer is still the level-triggered
            // liveness path for eligibility created by every other
            // transition. Competing drivers are ordered by the accepted-pool
            // write boundary and select their ticket inside the kernel Apply.
            Ok(_version) => {
                self.drive_pipeline_commits().await;
            }
            Err(PrePoolError::Stale(_)) => {}
            Err(PrePoolError::Public(error)) => {
                let reject = crate::component::pre_pool::pre_pool_reject(error);
                self.settle_pipeline_verify_failure(&lease, VerifyFailure::Rejected(reject))
                    .await;
            }
            Err(PrePoolError::Duplicate(hash)) => self.fail_tx_pool_generation(
                "verification completion produced a duplicate entry",
                &crate::process::TxPoolGenerationFault::PrePool(
                    PrePoolFault::UnexpectedTransitionDuplicate(hash),
                ),
            ),
            Err(PrePoolError::Fault(fault)) => self.fail_tx_pool_generation(
                "verification completion invariant failed",
                &crate::process::TxPoolGenerationFault::PrePool(fault),
            ),
        }
    }
}

impl JobHandler for VerifyHandler {
    type Job = VerifyLease;

    fn worker_name(&self) -> &'static str {
        "verify worker"
    }

    async fn is_queue_empty(&self) -> bool {
        self.service
            .pipeline
            .kernel
            .queue_is_empty(WorkLane::Verify)
    }

    async fn queue_ready(&self) -> Arc<tokio::sync::Notify> {
        self.service
            .pipeline
            .kernel
            .subscribe_verify(self.capability)
    }

    async fn pop_one(&mut self) -> Option<VerifyLease> {
        match self
            .service
            .pipeline
            .kernel
            .mutate_authoritative(|kernel| kernel.checkout_verify(self.capability))
        {
            Ok(lease) => lease,
            Err(error) => {
                self.service.fail_tx_pool_generation(
                    "verify checkout invariant failed",
                    &crate::process::TxPoolGenerationFault::PrePool(error.into_unexpected_fault()),
                );
                None
            }
        }
    }

    async fn process_one(&mut self, work: VerifyLease) {
        self.service
            .process_pipeline_verify_lease(work, &mut self.command_rx)
            .await;
    }
}

pub(crate) fn spawn_verify_workers(
    handle: &Handle,
    service: TxPoolService,
    command_rx: watch::Receiver<ChunkCommand>,
    signal: CancellationToken,
) -> Vec<tokio::task::JoinHandle<()>> {
    let worker_num = service.pool.tx_pool_config.max_tx_verify_workers.max(1);
    (0..worker_num)
        .map(|worker_id| {
            let capability = if worker_id == 0 && worker_num > 1 {
                WorkCapability::SmallCycleOnly
            } else {
                WorkCapability::Any
            };
            let runner = WorkerRunner::new(
                VerifyHandler {
                    service: service.clone(),
                    capability,
                    command_rx: command_rx.clone(),
                },
                command_rx.clone(),
                signal.child_token(),
            );
            handle.spawn(runner.run(worker_id))
        })
        .collect()
}
