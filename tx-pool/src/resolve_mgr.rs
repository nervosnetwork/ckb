//! Resolve stage of the tx-pool pipeline.
//!
//! Transactions that could not be resolved at the submission entry (missing
//! inputs / dependent on an in-flight tx) land in the ordered resolve queue.
//! A single `OrderedResolver` worker pops them in arrival order, retries
//! `pre_check`, and routes the result to `VerifyQueue` or the orphan pool.
//! Keeping this stage ordered reduces orphan-pool churn for dependent txs.

use crate::component::ordered_resolve_queue::OrderedResolveQueue;
use crate::component::verify_queue::VerifyQueue;
use crate::error::Reject;
use crate::resolved_tx::{ResolveJob, ResolvedTx};
use crate::service::TxPoolService;
use ckb_logger::{debug, error, info};
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use ckb_types::packed::{Byte32, ProposalShortId};
use futures_util::FutureExt;
use crate::util::panic_payload_to_string;
use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use tokio::sync::{RwLock, watch};
use tokio::task::JoinHandle;

/// Maximum number of times a local orphan transaction is re-enqueued for
/// retry when none of its missing parents are currently in the pipeline.
/// This bounds the work we do for genuinely unsatisfiable orphans while still
/// giving us enough slack to recover from the small race window between a
/// parent leaving the verify queue and being committed to the pool.
const MAX_LOCAL_ORPHAN_ATTEMPTS: u8 = 5;

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
    let (pre_check_ret, snapshot) = service.pre_check(&job.tx).await;

    match pre_check_ret {
        Ok((pre_resolve_tip, rtx, status, fee, tx_size)) => {
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

/// Single ordered resolver worker.
///
/// Processes transactions that the entry classifier could not resolve because
/// of missing inputs.  Keeping this worker single-threaded preserves arrival
/// ordering for dependent transactions.
pub(crate) struct OrderedResolver {
    ordered_queue: Arc<RwLock<OrderedResolveQueue>>,
    verify_queue: Arc<RwLock<VerifyQueue>>,
    command_rx: watch::Receiver<ChunkCommand>,
    service: TxPoolService,
    exit_signal: CancellationToken,
    status: ChunkCommand,
}

impl Clone for OrderedResolver {
    fn clone(&self) -> Self {
        Self {
            ordered_queue: Arc::clone(&self.ordered_queue),
            verify_queue: Arc::clone(&self.verify_queue),
            command_rx: self.command_rx.clone(),
            exit_signal: self.exit_signal.clone(),
            service: self.service.clone(),
            status: self.status.clone(),
        }
    }
}

impl OrderedResolver {
    pub fn new(
        service: TxPoolService,
        ordered_queue: Arc<RwLock<OrderedResolveQueue>>,
        verify_queue: Arc<RwLock<VerifyQueue>>,
        command_rx: watch::Receiver<ChunkCommand>,
        exit_signal: CancellationToken,
    ) -> Self {
        OrderedResolver {
            ordered_queue,
            verify_queue,
            command_rx,
            exit_signal,
            service,
            status: ChunkCommand::Resume,
        }
    }

    pub fn start(self, exit_tx: tokio::sync::mpsc::UnboundedSender<ResolveExit>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let exit = match AssertUnwindSafe(self.run()).catch_unwind().await {
                Ok(()) => ResolveExit::Stopped,
                Err(payload) => ResolveExit::Panicked {
                    message: panic_payload_to_string(payload.as_ref()),
                },
            };
            if let Err(err) = exit_tx.send(exit) {
                error!(
                    "failed to notify tx-pool ordered resolver exit: {:?}",
                    err.0
                );
            }
        })
    }

    async fn run(mut self) {
        let queue_ready = self.ordered_queue.read().await.subscribe();
        self.refresh_status();
        loop {
            tokio::select! {
                _ = self.exit_signal.cancelled() => {
                    break;
                }
                _ = self.command_rx.changed() => {
                    self.status = self.command_rx.borrow_and_update().to_owned();
                    self.process_inner().await;
                }
                _ = queue_ready.notified() => {
                    self.process_inner().await;
                }
            };
        }
    }

    fn refresh_status(&mut self) {
        self.status = self.command_rx.borrow().to_owned();
    }

    async fn process_inner(&mut self) {
        loop {
            if self.exit_signal.is_cancelled() {
                info!("Ordered resolver::process_inner exit_signal is cancelled");
                return;
            }
            self.refresh_status();
            if self.status != ChunkCommand::Resume {
                return;
            }
            if self.ordered_queue.read().await.is_empty() {
                return;
            }

            let job = {
                let mut queue = self.ordered_queue.write().await;
                queue.pop_front()
            };
            let Some(job) = job else {
                return;
            };

            match resolve_job(&self.service, job.clone()).await {
                ResolveStageResult::Ready(resolved) => {
                    self.push_to_verify_queue(resolved).await;
                }
                ResolveStageResult::Orphan(id, parents) => {
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
                            )
                            .await;
                    } else {
                        // Local transactions with missing inputs are normally rejected.
                        // The exception is when the missing parent is currently in the
                        // pipeline (e.g. a parent detached during a reorg that has not
                        // finished verification yet). In that case we put the child back
                        // into the ordered resolve queue so it can be retried once the
                        // parent lands in the pool.
                        let depends_on_pipeline = {
                            let ordered = self.ordered_queue.read().await;
                            if ordered.depends_on(&job.tx) {
                                true
                            } else {
                                let verify_queue = self.verify_queue.read().await;
                                verify_queue.depends_on(&job.tx)
                            }
                        };

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
                            let mut ordered = self.ordered_queue.write().await;
                            if let Err(reject) = ordered.add_tx(job) {
                                drop(ordered);
                                let (_ret, snapshot) = self
                                    .service
                                    .with_tx_pool_read_lock(|_tx_pool, snapshot| snapshot)
                                    .await;
                                self.service
                                    .after_process(tx, None, &snapshot, &Err(reject))
                                    .await;
                            }
                        } else {
                            let reject = first_unknown_input_reject(&job.tx);
                            let (_ret, snapshot) = self
                                .service
                                .with_tx_pool_read_lock(|_tx_pool, snapshot| snapshot)
                                .await;
                            self.service
                                .after_process(job.tx, None, &snapshot, &Err(reject))
                                .await;
                        }
                    }
                }
                ResolveStageResult::Reject(tx, reject) => {
                    let (_ret, snapshot) = self
                        .service
                        .with_tx_pool_read_lock(|_tx_pool, snapshot| snapshot)
                        .await;
                    self.service
                        .after_process(tx, job.remote, &snapshot, &Err(reject))
                        .await;
                }
            }
        }
    }

    async fn push_to_verify_queue(&self, resolved: ResolvedTx) {
        // Extract fields needed for after_process before resolved is consumed by add_tx.
        let tx = resolved.tx.clone();
        let remote = resolved.remote;
        let snapshot = Arc::clone(&resolved.snapshot);

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
            .after_process(tx, remote, &snapshot, &Err(reject))
            .await;
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
