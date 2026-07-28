//! Ordered resolution worker backed exclusively by the pre-pool kernel.

use super::runner::{ContinuationMode, JobHandler, WorkerRunner};
use crate::component::pre_pool::{
    DependencyKey, PrePoolError, ResolveLane, ResolveLease, VerifyCycleClass, VerifySchedule,
};
use crate::error::Reject;
use crate::process::PreCheckedTx;
use crate::resolved_tx::ResolvedTx;
use crate::service::TxPoolService;
use crate::service::pipeline_ops::ParentWaitOutcome;
use ckb_async_runtime::Handle;
use ckb_logger::debug;
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use ckb_types::core::FeeRate;
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::watch;

#[derive(Debug)]
pub(crate) enum ResolveStageResult {
    Ready(ResolvedTx),
    Orphan(BTreeSet<DependencyKey>),
    Reject(Reject),
}

pub(crate) async fn resolve_job(
    service: &TxPoolService,
    tx: ckb_types::core::TransactionView,
    source: crate::tx_source::TxSource,
    epoch: u64,
) -> ResolveStageResult {
    let tx_size = tx.data().serialized_size_in_block();
    let (pre_check_ret, _snapshot) = service.pre_check(&tx, tx_size).await;
    match pre_check_ret {
        Ok(PreCheckedTx {
            pre_resolve_tip,
            rtx,
            status,
            fee,
            tx_size,
            resident_size,
        }) => {
            debug!("resolve stage resolved tx {}", tx.proposal_short_id());
            ResolveStageResult::Ready(ResolvedTx {
                rtx,
                status,
                fee,
                tx_size,
                resident_size,
                pre_resolve_tip,
                source,
                epoch,
            })
        }
        Err(Reject::Resolve(ckb_types::core::error::OutPointError::Unknown(out_point))) => {
            ResolveStageResult::Orphan(BTreeSet::from([DependencyKey::Cell(
                crate::util::compact_packed(&out_point),
            )]))
        }
        Err(reject) => ResolveStageResult::Reject(reject),
    }
}

impl TxPoolService {
    async fn settle_pipeline_raw_lease(&self, lease: &ResolveLease, reject: Option<Reject>) {
        self.settle_pipeline_terminal(
            &lease.hash,
            reject,
            "current raw lease could not terminalize",
            |kernel, disposition| {
                if matches!(
                    disposition,
                    crate::component::pre_pool::ConflictDisposition::Retain
                ) {
                    kernel.park_resolve_conflict_or_terminalize(lease)
                } else {
                    kernel.terminalize_resolve(lease)
                }
            },
        )
        .await;
    }

    pub(crate) async fn process_pipeline_raw_lease(&self, lease: ResolveLease) {
        self.process_pipeline_raw_lease_inner(lease, ContinuationMode::Final)
            .await;
    }

    #[cfg_attr(
        feature = "profiling",
        tracing::instrument(
            name = "tx_pool.stage.resolve",
            target = "ckb_tx_pool_profile",
            level = "trace",
            skip_all
        )
    )]
    async fn process_pipeline_raw_lease_inner(
        &self,
        lease: ResolveLease,
        continuation: ContinuationMode,
    ) -> Option<ResolveLease> {
        let tx = lease.payload.tx.clone();
        let epoch = lease.payload.admitted_epoch;
        let current_source = self
            .pipeline
            .kernel
            .read(|kernel| kernel.source_by_hash(&lease.hash))?;
        let source = lease.payload.authoritative_source(current_source);

        if !self.is_pipeline_epoch_current(epoch) || self.is_recently_banned(source) {
            self.settle_pipeline_raw_lease(&lease, None).await;
            return None;
        }

        let resolved = resolve_job(self, tx.clone(), source, epoch).await;

        match resolved {
            ResolveStageResult::Ready(resolved) => {
                // Raw admission can name a dep-group cell but only a
                // successful resolver can name every expanded member.
                // Publish those producer hashes in the same kernel
                // transition as the resolved payload so later pool
                // removal invalidates it causally instead of waiting for
                // a stale final-commit failure.
                let resolved_dependencies = resolved
                    .rtx
                    .related_dep_out_points()
                    .map(|out_point| {
                        crate::component::pre_pool::DependencyKey::Cell(
                            crate::util::compact_packed(out_point),
                        )
                    })
                    .collect::<std::collections::BTreeSet<_>>();
                let charge_bytes = resolved.resident_size;
                let fee_rate = FeeRate::calculate(resolved.fee, resolved.tx_size as u64);
                let cycle_class = if source
                    .cycles()
                    .is_some_and(|cycles| cycles > self.pool.tx_pool_config.max_tx_verify_cycles)
                {
                    VerifyCycleClass::Large
                } else {
                    VerifyCycleClass::Small
                };
                let schedule = VerifySchedule::new(fee_rate.as_u64(), cycle_class);
                match self
                    .pipeline
                    .kernel
                    .mutate_authoritative(|kernel| match continuation {
                        ContinuationMode::Permit => kernel.complete_resolve_and_checkout(
                            &lease,
                            resolved,
                            charge_bytes,
                            schedule,
                            resolved_dependencies,
                        ),
                        ContinuationMode::Final => kernel.complete_resolve_without_checkout(
                            &lease,
                            resolved,
                            charge_bytes,
                            schedule,
                            resolved_dependencies,
                        ),
                    }) {
                    Ok(applied) => super::finish_continuation(
                        self,
                        applied,
                        "post-resolve continuation checkout failed",
                    ),
                    Err(PrePoolError::Stale(_)) => None,
                    Err(PrePoolError::Public(error)) => {
                        let reject = crate::component::pre_pool::pre_pool_reject(error);
                        self.settle_pipeline_raw_lease(&lease, Some(reject)).await;
                        None
                    }
                    Err(error @ (PrePoolError::Duplicate(_) | PrePoolError::Fault(_))) => {
                        self.fail_tx_pool_generation(
                            "raw completion invariant failed",
                            &crate::process::TxPoolGenerationFault::PrePool(
                                error.into_unexpected_fault(),
                            ),
                        );
                        None
                    }
                }
            }
            ResolveStageResult::Orphan(parents) => {
                match self.settle_raw_parent_wait(&lease, parents).await {
                    Some(ParentWaitOutcome::Parked) => {}
                    Some(ParentWaitOutcome::Requeued) => {}
                    Some(ParentWaitOutcome::Unavailable) => {
                        let reject = first_unknown_input_reject(&tx);
                        self.settle_pipeline_raw_lease(&lease, Some(reject)).await;
                    }
                    Some(ParentWaitOutcome::Rejected(reject)) => {
                        self.settle_pipeline_raw_lease(&lease, Some(reject)).await;
                    }
                    None => {}
                }
                None
            }
            ResolveStageResult::Reject(reject) => {
                self.settle_pipeline_raw_lease(&lease, Some(reject)).await;
                None
            }
        }
    }

    pub(crate) async fn process_pipeline_raw_lease_continuing(
        &self,
        lease: ResolveLease,
    ) -> Option<ResolveLease> {
        self.process_pipeline_raw_lease_inner(lease, ContinuationMode::Permit)
            .await
    }
}

#[derive(Clone)]
struct ResolveHandler {
    service: TxPoolService,
}

impl JobHandler for ResolveHandler {
    type Job = ResolveLease;

    fn worker_name(&self) -> &'static str {
        "ordered resolver"
    }

    async fn queue_ready(&self) -> Arc<tokio::sync::Notify> {
        self.service
            .pipeline
            .kernel
            .subscribe_resolve(ResolveLane::Ordered)
    }

    async fn pop_one(&mut self) -> Option<ResolveLease> {
        match self
            .service
            .pipeline
            .kernel
            .checkout_resolve(ResolveLane::Ordered)
        {
            Ok(lease) => lease,
            Err(error) => {
                self.service.fail_tx_pool_generation(
                    "ordered resolve checkout invariant failed",
                    &crate::process::TxPoolGenerationFault::PrePool(error.into_unexpected_fault()),
                );
                None
            }
        }
    }

    async fn next_deadline(&self) -> Option<tokio::time::Instant> {
        None
    }

    async fn process_one(&mut self, work: ResolveLease) -> Option<ResolveLease> {
        self.service
            .process_pipeline_raw_lease_continuing(work)
            .await
    }

    async fn process_final(&mut self, work: ResolveLease) {
        self.service.process_pipeline_raw_lease(work).await;
    }
}

pub(crate) fn spawn_ordered_resolver(
    handle: &Handle,
    service: TxPoolService,
    command_rx: watch::Receiver<ChunkCommand>,
    signal: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    handle.spawn(WorkerRunner::new(ResolveHandler { service }, command_rx, signal).run(0))
}

fn first_unknown_input_reject(tx: &ckb_types::core::TransactionView) -> Reject {
    let outpoint = tx.input_pts_iter().next().unwrap_or_default();
    Reject::Resolve(ckb_types::core::error::OutPointError::Unknown(outpoint))
}
