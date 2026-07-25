//! Ordered resolution worker backed exclusively by the pipeline coordinator.

use crate::component::pipeline_coordinator::{
    QueueKind, RawStage, RawWorkLease, TerminalDisposition, VerifySchedule, WorkerCapability,
};
use crate::component::pipeline_runtime::PipelineRawTx;
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
use ckb_types::packed::Byte32;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

#[derive(Debug)]
pub(crate) enum ResolveStageResult {
    Ready(ResolvedTx),
    Orphan(HashSet<Byte32>),
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
        Err(reject) if crate::util::is_missing_input(&reject) => {
            let parents = match &reject {
                Reject::Resolve(ckb_types::core::error::OutPointError::Unknown(outpoint)) => {
                    HashSet::from([outpoint.tx_hash()])
                }
                _ => job.tx.unique_parents(),
            };
            ResolveStageResult::Orphan(parents)
        }
        Err(reject) => ResolveStageResult::Reject(reject),
    }
}

impl TxPoolService {
    async fn settle_pipeline_raw_lease(
        &self,
        lease: &RawWorkLease<PipelineRawTx>,
        disposition: TerminalDisposition,
        reject: Option<Reject>,
    ) {
        let permit = self
            .reserve_required_effects(
                Self::pipeline_outcome_effect_bytes(reject.as_ref()),
                "raw terminal effect reservation failed",
            )
            .await;
        let mut tx_pool = if reject.is_some() {
            Some(self.pool.tx_pool.write().await)
        } else {
            None
        };
        let mut banned_peer = None;
        let terminal = self.pipeline.runtime.mutate_lease(
            "current raw lease could not terminalize",
            |coordinator| {
                let result = coordinator.terminalize_raw(lease, disposition);
                if let Ok(record) = &result {
                    banned_peer = self.journal_pipeline_outcome(
                        permit,
                        record,
                        reject.as_ref(),
                        tx_pool.as_deref_mut(),
                    );
                }
                result
            },
        );
        drop(tx_pool);
        if terminal.is_none() {
            return;
        }
        if let Some(peer) = banned_peer {
            self.remove_banned_peer_entries(peer).await;
        }
    }

    pub(crate) async fn process_pipeline_raw_lease(&self, lease: RawWorkLease<PipelineRawTx>) {
        let tx = lease.payload.tx.clone();
        let epoch = lease.payload.admitted_epoch;
        let Some(current_source) = self
            .pipeline
            .runtime
            .read(|coordinator| coordinator.view(&lease.hash).map(|view| view.source))
        else {
            return;
        };
        let source = self
            .pipeline
            .runtime
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
                        .map(|out_point| crate::util::compact_packed(&out_point.tx_hash()))
                        .collect::<HashSet<_>>();
                    let charge_bytes = resolved.resident_size;
                    let fee_rate = FeeRate::calculate(resolved.fee, resolved.tx_size as u64);
                    let schedule = VerifySchedule::new(
                        fee_rate.as_u64(),
                        source.cycles().is_some_and(|cycles| {
                            cycles > self.pool.tx_pool_config.max_tx_verify_cycles
                        }),
                    );
                    let permit = self
                        .reserve_required_effects(
                            Self::pipeline_terminal_effect_bytes(
                                crate::constants::MAX_POOL_MUTATION_CANDIDATES.saturating_add(1),
                            ),
                            "raw completion effect reservation failed",
                        )
                        .await;
                    match self.pipeline.runtime.mutate(|coordinator| {
                        let result = coordinator.complete_raw_with_dependencies(
                            &lease,
                            resolved,
                            charge_bytes,
                            schedule,
                            resolved_dependencies,
                        );
                        if let Ok((_version, terminal)) = &result {
                            self.journal_pipeline_terminal_records(permit, terminal);
                        }
                        result
                    }) {
                        Ok((_version, _terminal)) => {}
                        Err(error) if error.is_stale_lease() => {}
                        Err(error) => {
                            let reject = self.pipeline.runtime.reject_or_fail(
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
                    let permit = self
                        .reserve_required_effects(
                            Self::unknown_parents_effect_bytes(parents.len()),
                            "raw parent-wait effect reservation failed",
                        )
                        .await;
                    match self.settle_raw_parent_wait(&lease, parents, permit).await {
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
            if self.pipeline.runtime.is_failed() {
                return;
            }
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
    type Job = RawWorkLease<PipelineRawTx>;
    type Exit = ResolveExit;

    fn worker_name(&self) -> &'static str {
        "ordered resolver"
    }

    async fn is_queue_empty(&self) -> bool {
        self.service
            .pipeline
            .runtime
            .queue_is_empty(QueueKind::Resolve)
    }

    async fn queue_ready(&self) -> Arc<tokio::sync::Notify> {
        self.service
            .pipeline
            .runtime
            .subscribe(QueueKind::Resolve, WorkerCapability::Any)
    }

    async fn pop_one(&mut self) -> Option<RawWorkLease<PipelineRawTx>> {
        self.service
            .pipeline
            .runtime
            .checkout_raw(RawStage::Resolve)
    }

    async fn next_deadline(&self) -> Option<tokio::time::Instant> {
        None
    }

    async fn process_one(&mut self, work: RawWorkLease<PipelineRawTx>) {
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
