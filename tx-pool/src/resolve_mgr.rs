//! Resolve stage of the tx-pool pipeline.
//!
//! Transactions that could not be resolved at the submission entry (missing
//! inputs / dependent on an in-flight tx) land in the ordered resolve queue.
//! A single `ResolveHandler` worker pops them in arrival order, retries
//! `pre_check`, and routes the result to `VerifyQueue` or the orphan pool.
//! Keeping this stage ordered reduces orphan-pool churn for dependent txs.

use crate::component::pipeline_queue::PipelineQueue;
use crate::error::Reject;
use crate::process::PreCheckedTx;
use crate::resolved_tx::{ResolveJob, ResolvedTx};
use crate::service::TxPoolService;
use crate::tx_source::TxSource;
use crate::worker::{JobHandler, WorkerOutcome, WorkerRunner};
use ckb_logger::debug;
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use ckb_types::packed::{Byte32, ProposalShortId};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

/// Maximum number of times a local orphan transaction is re-enqueued for
/// retry when none of its missing parents are currently in the pipeline.
/// This bounds the work we do for genuinely unsatisfiable orphans while still
/// giving us enough slack to recover from the small race window between a
/// parent leaving the verify queue and being committed to the pool.
pub(crate) const MAX_LOCAL_ORPHAN_ATTEMPTS: u8 = 5;

/// Delay before re-enqueueing a local orphan whose missing parents are still
/// in flight. Without this delay the single ordered resolver would retry the
/// same job in a tight loop until the parent lands, burning CPU and snapshot
/// I/O. The delay is short enough that it does not materially delay
/// confirmation once the parent is accepted.
pub(crate) const LOCAL_ORPHAN_RETRY_DELAY: Duration = Duration::from_millis(50);

/// Result of attempting to resolve one transaction.
#[derive(Debug)]
pub(crate) enum ResolveStageResult {
    /// Transaction resolved successfully and is ready for verification.
    Ready(ResolvedTx),
    /// Transaction has unknown parent transactions and should be sent to the
    /// orphan pool (remote) or rejected (local).
    Orphan(ProposalShortId, HashSet<Byte32>),
    /// Transaction is invalid and should be rejected.
    Reject(ckb_types::core::TransactionView, Reject),
}

/// Run `pre_check` for a single resolve job and map the result to a uniform
/// stage result.
///
/// This helper is used both by the entry classifier and by the ordered
/// resolver, so the resolve logic is not duplicated.
pub(crate) async fn resolve_job(service: &TxPoolService, job: ResolveJob) -> ResolveStageResult {
    let id = job.tx.proposal_short_id();
    let tx_size = job.tx.data().serialized_size_in_block();
    let (pre_check_ret, snapshot) = service.pre_check(&job.tx, tx_size).await;

    match pre_check_ret {
        Ok(PreCheckedTx {
            pre_resolve_tip,
            rtx,
            status,
            fee,
            tx_size,
        }) => {
            debug!("resolve stage resolved tx {}", id);
            ResolveStageResult::Ready(ResolvedTx {
                tx: job.tx,
                rtx,
                status,
                fee,
                tx_size,
                pre_resolve_tip,
                snapshot,
                source: job.source,
            })
        }
        Err(reject) => {
            if crate::util::is_missing_input(&reject) {
                let parents = job.tx.unique_parents();
                ResolveStageResult::Orphan(id, parents)
            } else {
                ResolveStageResult::Reject(job.tx, reject)
            }
        }
    }
}

#[derive(Clone)]
struct ResolveHandler {
    service: TxPoolService,
}

impl JobHandler for ResolveHandler {
    type Job = ResolveJob;
    type Exit = ResolveExit;

    fn worker_name(&self) -> &'static str {
        "ordered resolver"
    }

    async fn is_queue_empty(&self) -> bool {
        self.service
            .queues
            .ordered_resolve_queue
            .read()
            .await
            .is_empty()
    }

    async fn queue_ready(&self) -> Arc<tokio::sync::Notify> {
        self.service
            .queues
            .ordered_resolve_queue
            .read()
            .await
            .subscribe()
    }

    async fn pop_one(&mut self) -> Option<ResolveJob> {
        self.service
            .queues
            .ordered_resolve_queue
            .write()
            .await
            .pop_front()
    }

    async fn process_one(&mut self, job: ResolveJob) {
        match resolve_job(&self.service, job.clone()).await {
            ResolveStageResult::Ready(resolved) => {
                self.push_to_verify_queue(resolved).await;
            }
            ResolveStageResult::Orphan(id, parents) => {
                self.handle_orphan(job, id, parents).await;
            }
            ResolveStageResult::Reject(tx, reject) => {
                self.handle_reject(tx, job.source, reject).await;
            }
        }
    }

    fn make_exit(&self, outcome: WorkerOutcome) -> ResolveExit {
        match outcome {
            WorkerOutcome::Stopped => ResolveExit::Stopped,
            WorkerOutcome::Panicked(message) => ResolveExit::Panicked { message },
        }
    }
}

impl ResolveHandler {
    async fn push_to_verify_queue(&self, resolved: ResolvedTx) {
        let tx_hash = resolved.tx.hash();
        // The verify queue has a single entry point so the in-flight RBF gate
        // and the after_process side effects cannot be bypassed or duplicated.
        match self.service.enqueue_resolved_tx(resolved).await {
            Ok(true) => {}
            Ok(false) => {
                debug!("resolved tx {} already in verify queue", tx_hash);
            }
            // enqueue_resolved_tx already ran after_process for the rejection.
            Err(_) => {}
        }
    }

    async fn handle_orphan(
        &mut self,
        job: ResolveJob,
        id: ProposalShortId,
        parents: HashSet<Byte32>,
    ) {
        if let Some(peer) = job.source.peer() {
            debug!(
                "ordered resolve stage orphan tx {} from peer {}, parents {:?}",
                id, peer, parents
            );
            self.service
                .handle_missing_input_orphan(job.tx.clone(), job.source, parents)
                .await;
            return;
        }

        // Local transactions with missing inputs are normally rejected.
        // The exception is when every missing parent is currently in the
        // pipeline (pre-check, ordered, verify, or already committed to
        // the pool). In that case we put the child back into the ordered
        // resolve queue so it can be retried once the parent is ready.
        //
        // We require *all* missing parents to be in flight: if one parent is
        // in the pool but another is permanently missing, retrying forever
        // would never succeed. In that case we burn an attempt so the orphan
        // is eventually rejected.
        let all_missing_parents_in_flight =
            self.service.all_missing_parents_in_flight(&parents).await;

        if all_missing_parents_in_flight {
            // The parents are still in flight, so this orphan will resolve
            // once they land. Re-enqueue it after a short delay instead of
            // retrying immediately: the single ordered resolver would
            // otherwise spin on this job until the parent is accepted.
            //
            // Note: during the delay window the job lives only in this spawned
            // task, so `remove_tx` cannot see it and it may be re-added after
            // an explicit removal. The window is small (50ms) and harmless —
            // the re-added tx goes through the normal pipeline again.
            debug!(
                "ordered resolve stage local orphan {} delayed re-enqueue (parents in flight)",
                id
            );
            let queues = Arc::clone(&self.service.queues);
            let service = self.service.clone();
            tokio::spawn(async move {
                tokio::time::sleep(LOCAL_ORPHAN_RETRY_DELAY).await;
                let tx = job.tx.clone();
                let source = job.source;
                let mut ordered = queues.ordered_resolve_queue.write().await;
                if let Err(reject) = ordered.add_tx(job) {
                    drop(ordered);
                    service.reject_with_after_process(tx, source, reject).await;
                }
            });
        } else if job.attempts < MAX_LOCAL_ORPHAN_ATTEMPTS {
            let mut job = job;
            job.attempts += 1;
            debug!(
                "ordered resolve stage local orphan {} re-enqueue (attempt {})",
                id, job.attempts
            );
            let tx = job.tx.clone();
            let source = job.source;
            let mut ordered = self.service.queues.ordered_resolve_queue.write().await;
            if let Err(reject) = ordered.add_tx(job) {
                drop(ordered);
                self.service
                    .reject_with_after_process(tx, source, reject)
                    .await;
            }
        } else {
            let reject = first_unknown_input_reject(&job.tx);
            self.service
                .reject_with_after_process(job.tx, job.source, reject)
                .await;
        }
    }

    async fn handle_reject(
        &self,
        tx: ckb_types::core::TransactionView,
        source: TxSource,
        reject: Reject,
    ) {
        self.service
            .reject_with_after_process(tx, source, reject)
            .await;
    }
}

/// Single ordered resolver worker runner.
///
/// Processes transactions that the entry classifier could not resolve because
/// of missing inputs. Keeping this worker single-threaded preserves arrival
/// ordering for dependent transactions.
pub(crate) struct OrderedResolver {
    runner: WorkerRunner<ResolveHandler>,
}

impl OrderedResolver {
    pub fn new(
        service: TxPoolService,
        command_rx: watch::Receiver<ChunkCommand>,
        exit_signal: CancellationToken,
    ) -> Self {
        let handler = ResolveHandler { service };
        Self {
            runner: WorkerRunner::new(handler, command_rx, exit_signal),
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
