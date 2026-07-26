//! Ordered resolution worker backed exclusively by the pre-pool kernel.

use crate::component::pre_pool::{
    DependencyKey, ResolveLane, ResolveLease, TerminalDisposition, VerifySchedule, WorkCapability,
    WorkLane,
};
use crate::error::Reject;
use crate::process::PreCheckedTx;
use crate::resolved_tx::{ResolveJob, ResolvedTx};
use crate::service::TxPoolService;
use crate::service::pipeline_ops::ParentWaitOutcome;
use crate::worker::{JobHandler, WorkerOutcome, WorkerRunner};
use ckb_logger::{debug, error};
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use ckb_types::core::FeeRate;
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

#[derive(Debug)]
pub(crate) enum ResolveStageResult {
    Ready(ResolvedTx),
    Orphan(BTreeSet<DependencyKey>),
    Reject(Reject),
}

pub(crate) async fn resolve_job(service: &TxPoolService, job: ResolveJob) -> ResolveStageResult {
    let tx_size = job.tx.data().serialized_size_in_block();
    let (pre_check_ret, _snapshot) = service.pre_check(&job.tx, tx_size).await;
    match pre_check_ret {
        Ok(PreCheckedTx {
            pre_resolve_tip,
            rtx,
            status,
            fee,
            tx_size,
            resident_size,
        }) => {
            debug!("resolve stage resolved tx {}", job.tx.proposal_short_id());
            ResolveStageResult::Ready(ResolvedTx {
                tx: job.tx,
                rtx,
                status,
                fee,
                tx_size,
                resident_size,
                pre_resolve_tip,
                source: job.source,
                epoch: job.epoch,
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
    async fn settle_pipeline_raw_lease(
        &self,
        lease: &ResolveLease,
        disposition: TerminalDisposition,
        reject: Option<Reject>,
    ) {
        self.settle_pipeline_terminal(
            &lease.hash,
            reject,
            "current raw lease could not terminalize",
            |coordinator, retain_conflict| {
                if retain_conflict {
                    coordinator.park_conflict_or_terminalize(
                        &lease.hash,
                        lease.version,
                        crate::component::pre_pool::PrePoolLocation::ResolveLeased,
                    )
                } else {
                    coordinator.terminalize_resolve(lease, disposition)
                }
            },
        )
        .await;
    }

    pub(crate) async fn process_pipeline_raw_lease(&self, lease: ResolveLease) {
        let tx = lease.payload.tx.clone();
        let epoch = lease.payload.admitted_epoch;
        let Some(current_source) = self
            .pipeline
            .kernel
            .read(|coordinator| coordinator.source_by_hash(&lease.hash))
        else {
            return;
        };
        let source = self
            .pipeline
            .kernel
            .require_authoritative_source(&lease.payload, current_source);

        if !self.is_pipeline_epoch_current(epoch) || self.is_recently_banned(source) {
            self.settle_pipeline_raw_lease(&lease, TerminalDisposition::Internal, None)
                .await;
            return;
        }

        let job = ResolveJob::new_at(tx.clone(), source, epoch);
        let outcome = crate::worker::catch_job_panic(async {
            match resolve_job(self, job).await {
                ResolveStageResult::Ready(resolved) => {
                    // Raw admission can name a dep-group cell but only a
                    // successful resolver can name every expanded member.
                    // Publish those producer hashes in the same coordinator
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
                    let schedule = VerifySchedule::new(
                        fee_rate.as_u64(),
                        source.cycles().is_some_and(|cycles| {
                            cycles > self.pool.tx_pool_config.max_tx_verify_cycles
                        }),
                    );
                    match self.pipeline.kernel.mutate(|coordinator| {
                        coordinator.complete_resolve(
                            &lease,
                            resolved,
                            charge_bytes,
                            schedule,
                            resolved_dependencies,
                        )
                    }) {
                        Ok(_version) => {}
                        Err(error) if error.is_stale_lease() => {}
                        Err(error) => {
                            let reject = self.pipeline.kernel.reject_or_fail(
                                "raw completion violated coordinator invariants",
                                error,
                            );
                            let public_reject =
                                (!matches!(reject, Reject::Full(_))).then_some(reject);
                            self.settle_pipeline_raw_lease(
                                &lease,
                                TerminalDisposition::Rejected,
                                public_reject,
                            )
                            .await;
                        }
                    }
                }
                ResolveStageResult::Orphan(parents) => {
                    match self.settle_raw_parent_wait(&lease, parents).await {
                        Some(ParentWaitOutcome::Parked) => {}
                        Some(ParentWaitOutcome::Requeued) => {}
                        Some(ParentWaitOutcome::Unavailable) => {
                            let reject = first_unknown_input_reject(&tx);
                            self.settle_pipeline_raw_lease(
                                &lease,
                                TerminalDisposition::Rejected,
                                Some(reject),
                            )
                            .await;
                        }
                        Some(ParentWaitOutcome::Rejected(reject)) => {
                            self.settle_pipeline_raw_lease(
                                &lease,
                                TerminalDisposition::Rejected,
                                Some(reject),
                            )
                            .await;
                        }
                        None => {}
                    }
                }
                ResolveStageResult::Reject(reject) => {
                    self.settle_pipeline_raw_lease(
                        &lease,
                        TerminalDisposition::Rejected,
                        Some(reject),
                    )
                    .await;
                }
            }
        })
        .await;

        if let Err(message) = outcome {
            error!("tx-pool raw worker panicked on {}: {}", lease.hash, message);
            self.settle_pipeline_raw_lease(&lease, TerminalDisposition::Internal, None)
                .await;
        }
    }
}

#[derive(Clone)]
struct ResolveHandler {
    service: TxPoolService,
}

impl JobHandler for ResolveHandler {
    type Job = ResolveLease;
    type Exit = ResolveExit;

    fn worker_name(&self) -> &'static str {
        "ordered resolver"
    }

    async fn is_queue_empty(&self) -> bool {
        self.service
            .pipeline
            .kernel
            .queue_is_empty(WorkLane::Resolve)
    }

    async fn queue_ready(&self) -> Arc<tokio::sync::Notify> {
        self.service
            .pipeline
            .kernel
            .subscribe(WorkLane::Resolve, WorkCapability::Any)
    }

    async fn pop_one(&mut self) -> Option<ResolveLease> {
        self.service
            .pipeline
            .kernel
            .checkout_resolve(ResolveLane::Ordered)
    }

    async fn next_deadline(&self) -> Option<tokio::time::Instant> {
        None
    }

    async fn process_one(&mut self, work: ResolveLease) {
        self.service.process_pipeline_raw_lease(work).await;
    }

    fn make_exit(&self, outcome: WorkerOutcome) -> ResolveExit {
        match outcome {
            WorkerOutcome::Stopped => ResolveExit::Stopped,
            WorkerOutcome::Panicked(message) => ResolveExit::Panicked { message },
        }
    }
}

pub(crate) struct OrderedResolver {
    runner: WorkerRunner<ResolveHandler>,
}

impl OrderedResolver {
    pub fn new(
        service: TxPoolService,
        command_rx: watch::Receiver<ChunkCommand>,
        exit_signal: CancellationToken,
    ) -> Self {
        Self {
            runner: WorkerRunner::new(ResolveHandler { service }, command_rx, exit_signal),
        }
    }

    pub fn start(self, exit_tx: mpsc::UnboundedSender<(usize, ResolveExit)>) -> JoinHandle<()> {
        self.runner.start(0, exit_tx)
    }
}

#[derive(Debug)]
pub(crate) enum ResolveExit {
    Stopped,
    Panicked { message: String },
}

fn first_unknown_input_reject(tx: &ckb_types::core::TransactionView) -> Reject {
    let outpoint = tx.input_pts_iter().next().unwrap_or_default();
    Reject::Resolve(ckb_types::core::error::OutPointError::Unknown(outpoint))
}
