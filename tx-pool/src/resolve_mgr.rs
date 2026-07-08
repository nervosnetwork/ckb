//! Resolve stage of the tx-pool pipeline.
//!
//! Transactions that could not be resolved at the submission entry (missing
//! inputs / dependent on an in-flight tx) land in the ordered resolve queue.
//! A single `ResolveHandler` worker pops them in arrival order, retries
//! `pre_check`, and routes the result to `VerifyQueue` or the orphan pool.
//! Keeping this stage ordered reduces orphan-pool churn for dependent txs.

use crate::component::ordered_resolve_queue::OrderedResolveQueue;
use crate::component::pipeline_queue::PipelineQueue;
use crate::component::verify_queue::VerifyQueue;
use crate::error::Reject;
use crate::process::PreCheckedTx;
use crate::resolved_tx::{ResolveJob, ResolvedTx};
use crate::service::TxPoolService;
use crate::worker::{JobHandler, WorkerOutcome, WorkerRunner};
use ckb_logger::debug;
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use ckb_types::packed::{Byte32, ProposalShortId};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc, watch};
use tokio::task::JoinHandle;

/// Maximum number of times a local orphan transaction is re-enqueued for
/// retry when none of its missing parents are currently in the pipeline.
/// This bounds the work we do for genuinely unsatisfiable orphans while still
/// giving us enough slack to recover from the small race window between a
/// parent leaving the verify queue and being committed to the pool.
pub(crate) const MAX_LOCAL_ORPHAN_ATTEMPTS: u8 = 5;

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
                remote: job.remote,
                is_proposal_tx: job.is_proposal_tx,
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
    ordered_queue: Arc<RwLock<OrderedResolveQueue>>,
    verify_queue: Arc<RwLock<VerifyQueue>>,
    service: TxPoolService,
}

impl JobHandler for ResolveHandler {
    type Job = ResolveJob;
    type Exit = ResolveExit;

    fn worker_name(&self) -> &'static str {
        "ordered resolver"
    }

    async fn is_queue_empty(&self) -> bool {
        self.ordered_queue.read().await.is_empty()
    }

    async fn queue_ready(&self) -> Arc<tokio::sync::Notify> {
        self.ordered_queue.read().await.subscribe()
    }

    async fn pop_one(&mut self) -> Option<ResolveJob> {
        self.ordered_queue.write().await.pop_front()
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
                self.handle_reject(tx, job.remote, reject, job.is_proposal_tx)
                    .await;
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
        // Extract fields needed for after_process before resolved is consumed by add_tx.
        let tx = resolved.tx.clone();
        let remote = resolved.remote;
        let snapshot = Arc::clone(&resolved.snapshot);
        let is_proposal_tx = resolved.is_proposal_tx;

        let reject = {
            let mut queue = self.verify_queue.write().await;
            match queue.add_tx(resolved) {
                Ok(true) => return,
                Ok(false) => {
                    debug!("resolved tx {} already in verify queue", tx.hash());
                    return;
                }
                Err(reject) => reject,
            }
        };
        // verify_queue lock released before after_process
        self.service
            .after_process(tx, remote, &snapshot, &Err(reject), is_proposal_tx)
            .await;
    }

    async fn handle_orphan(
        &mut self,
        job: ResolveJob,
        id: ProposalShortId,
        parents: HashSet<Byte32>,
    ) {
        if let Some((declared_cycle, peer)) = job.remote {
            debug!(
                "ordered resolve stage orphan tx {} from peer {}, parents {:?}",
                id, peer, parents
            );
            self.service
                .handle_missing_input_orphan(
                    job.tx.clone(),
                    peer,
                    declared_cycle,
                    parents,
                    job.is_proposal_tx,
                )
                .await;
            return;
        }

        // Local transactions with missing inputs are normally rejected.
        // The exception is when the missing parent is currently in the
        // pipeline (pre-check, ordered, verify, or already committed to
        // the pool). In that case we put the child back into the ordered
        // resolve queue so it can be retried once the parent is ready.
        let depends_on_pipeline = self.depends_on_pipeline(&job.tx, &parents).await;

        // If the missing parent is still in the in-flight pipeline,
        // keep retrying without burning an attempt. The child will be
        // re-resolved once the parent is committed to the pool.
        if depends_on_pipeline || job.attempts < MAX_LOCAL_ORPHAN_ATTEMPTS {
            let mut job = job;
            if !depends_on_pipeline {
                job.attempts += 1;
            }
            debug!(
                "ordered resolve stage local orphan {} re-enqueue (attempt {}, depends_on_pipeline: {})",
                id, job.attempts, depends_on_pipeline
            );
            let tx = job.tx.clone();
            let is_proposal_tx = job.is_proposal_tx;
            let mut ordered = self.ordered_queue.write().await;
            if let Err(reject) = ordered.add_tx(job) {
                drop(ordered);
                self.service
                    .reject_with_after_process(tx, None, reject, is_proposal_tx)
                    .await;
            }
        } else {
            let reject = first_unknown_input_reject(&job.tx);
            self.service
                .reject_with_after_process(job.tx, None, reject, job.is_proposal_tx)
                .await;
        }
    }

    async fn handle_reject(
        &self,
        tx: ckb_types::core::TransactionView,
        remote: Option<(ckb_types::core::Cycle, ckb_network::PeerIndex)>,
        reject: Reject,
        is_proposal_tx: bool,
    ) {
        self.service
            .reject_with_after_process(tx, remote, reject, is_proposal_tx)
            .await;
    }

    async fn depends_on_pipeline(
        &self,
        tx: &ckb_types::core::TransactionView,
        parents: &HashSet<Byte32>,
    ) -> bool {
        if self.service.depends_on_pipeline(tx).await {
            return true;
        }

        // A parent may have left the verify queue and be in the middle of
        // being committed to the pool. Treat an in-pool parent as still
        // in-flight so the child does not burn an attempt while waiting.
        let pool = self.service.tx_pool.read().await;
        parents.iter().any(|parent_hash| {
            pool.contains_proposal_id(&ProposalShortId::from_tx_hash(parent_hash))
        })
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
        ordered_queue: Arc<RwLock<OrderedResolveQueue>>,
        verify_queue: Arc<RwLock<VerifyQueue>>,
        command_rx: watch::Receiver<ChunkCommand>,
        exit_signal: CancellationToken,
    ) -> Self {
        let handler = ResolveHandler {
            ordered_queue,
            verify_queue,
            service,
        };
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
