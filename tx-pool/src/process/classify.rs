//! Entry classification: the path a transaction takes from submission to
//! being queued for verification.
//!
//! `pre_check` resolves the transaction and computes its fee (chain-only
//! fast path, falling back to the pool-overlay locked path), dependent
//! transactions are routed to the ordered resolve queue in arrival order,
//! and `process_tx_direct` is the direct per-tx entry point shared by RPC,
//! tests and reorg recovery. Split out of `process/mod.rs`.

use super::{get_tx_status, make_pre_checked_tx, resolve_tx};
use crate::component::pipeline_queue::PipelineQueue;
use crate::component::pre_check_queue::PreCheckJob;
use crate::error::Reject;
use crate::process::PreCheckedTx;
use crate::tx_source::TxSource;
use crate::util::{check_tx_fee, check_tx_fee_with_min_fee_rate, check_txid_collision};
use ckb_logger::error;
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_types::core::error::OutPointError;
use ckb_types::core::{TransactionView, cell::resolve_transaction};
use ckb_verification::cache::Completed;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::watch;

/// Routing decision from [`TxPoolService::check_and_route_dependent`].
#[derive(Debug)]
enum RouteDecision {
    /// The tx does not depend on any in-flight pipeline tx.
    Independent,
    /// The tx was enqueued in the ordered resolve queue.
    Enqueued,
    /// The tx is a duplicate of an already-queued tx.
    Duplicate,
}

impl super::TxPoolService {
    fn stale_pipeline_reject() -> Reject {
        Reject::Internal("tx-pool pipeline generation invalidated by clear".to_string())
    }

    async fn ensure_current_or_terminal(
        &self,
        tx: &TransactionView,
        source: TxSource,
        epoch: u64,
    ) -> Result<(), Reject> {
        if self.is_pipeline_epoch_current(epoch) {
            Ok(())
        } else {
            self.terminal_internal(tx.clone(), source).await;
            Err(Self::stale_pipeline_reject())
        }
    }

    pub(crate) async fn pre_check(
        &self,
        tx: &TransactionView,
        tx_size: usize,
    ) -> (Result<PreCheckedTx, Reject>, Arc<Snapshot>) {
        // Fast path: for transactions whose inputs and cell deps all come from the
        // chain (not from any tx currently in the pool), we can resolve and compute
        // the fee without holding the tx_pool read lock.  We only take the lock
        // briefly to check for txid collisions.
        let (collision, snapshot) = self
            .read_tx_pool_with_snapshot(|tx_pool, _snapshot| {
                check_txid_collision(tx_pool, tx).err()
            })
            .await;
        if let Some(reject) = collision {
            return (Err(reject), snapshot);
        }

        let short_id = tx.proposal_short_id();
        let mut seen_inputs =
            HashSet::with_capacity(tx.inputs().len().saturating_add(tx.cell_deps().len()));
        match resolve_transaction(
            tx.clone(),
            &mut seen_inputs,
            snapshot.as_ref(),
            snapshot.as_ref(),
        ) {
            Ok(rtx) => {
                let rtx = Arc::new(rtx);
                let fee = match check_tx_fee_with_min_fee_rate(
                    &snapshot,
                    &rtx,
                    tx_size,
                    self.pool.tx_pool_config.min_fee_rate,
                ) {
                    Ok(fee) => fee,
                    Err(reject) => return (Err(reject), snapshot),
                };
                let status = get_tx_status(&snapshot, &short_id);
                (
                    Ok(make_pre_checked_tx(
                        snapshot.tip_hash(),
                        rtx,
                        status,
                        fee,
                        tx_size,
                    )),
                    snapshot,
                )
            }
            Err(OutPointError::Unknown(_)) => {
                // At least one input/cell dep is not in the chain snapshot.  It may
                // be an output of a tx currently in the pool, so fall back to the
                // locked path which can resolve through the pool.
                self.pre_check_with_pool_lock(tx, tx_size).await
            }
            Err(err) => (Err(Reject::Resolve(err)), snapshot),
        }
    }

    async fn pre_check_with_pool_lock(
        &self,
        tx: &TransactionView,
        tx_size: usize,
    ) -> (Result<PreCheckedTx, Reject>, Arc<Snapshot>) {
        let (ret, snapshot) = self
            .read_tx_pool_with_snapshot(|tx_pool, snapshot| {
                let tip_hash = snapshot.tip_hash();

                // Same txid means exactly the same transaction, including inputs, outputs, witnesses, etc.
                // It's also not possible for RBF, reject it directly
                check_txid_collision(tx_pool, tx)?;

                // Try normal path first, if double-spending check success we don't need RBF check
                // this make sure RBF won't introduce extra performance cost for hot path
                let res = resolve_tx(tx_pool, &snapshot, tx.clone(), false);
                match res {
                    Ok((rtx, status)) => {
                        let fee = check_tx_fee(tx_pool, &snapshot, &rtx, tx_size)?;
                        Ok(make_pre_checked_tx(tip_hash, rtx, status, fee, tx_size))
                    }
                    Err(Reject::Resolve(OutPointError::Dead(out))) => {
                        let (rtx, status) = resolve_tx(tx_pool, &snapshot, tx.clone(), true)?;
                        let fee = check_tx_fee(tx_pool, &snapshot, &rtx, tx_size)?;
                        let conflicts = tx_pool.pool_map.find_conflict_outpoint(tx);
                        if conflicts.is_none() {
                            // this mean one input's outpoint is dead, but there is no direct conflicted tx in tx_pool
                            // we should reject it directly and don't need to put it into conflicts pool
                            error!(
                                "{} is resolved as Dead, but there is no direct conflicted tx",
                                rtx.transaction.proposal_short_id()
                            );
                            return Err(Reject::Resolve(OutPointError::Dead(out)));
                        }
                        // we also return Ok here, so that the entry will be continue to be verified before submit
                        // we only want to put it into conflicts pool after the verification stage passed
                        // then we will double-check conflicts txs in `submit_entry`

                        Ok(make_pre_checked_tx(tip_hash, rtx, status, fee, tx_size))
                    }
                    Err(err) => Err(err),
                }
            })
            .await;
        (ret, snapshot)
    }

    pub(crate) async fn process_tx_direct_outcome(
        &self,
        tx: TransactionView,
        source: TxSource,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Result<super::submit::VerifySubmitOutcome, Reject> {
        let epoch = self.current_pipeline_epoch()?;
        let tx_size = tx.data().serialized_size_in_block();
        let (ret, snapshot) = self.pre_check(&tx, tx_size).await;

        let PreCheckedTx {
            pre_resolve_tip,
            rtx,
            status,
            fee,
            tx_size,
        } = ret?;

        self.verify_and_submit_core(
            crate::resolved_tx::ResolvedTx {
                tx,
                rtx,
                status,
                fee,
                tx_size,
                pre_resolve_tip,
                snapshot,
                source,
                epoch,
                verified: None,
                resident_permit: None,
            },
            command_rx,
        )
        .await
    }

    pub(crate) async fn process_tx_direct(
        &self,
        tx: TransactionView,
        source: TxSource,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Result<Completed, Reject> {
        match self.process_tx_direct_outcome(tx, source, command_rx).await {
            Ok(super::submit::VerifySubmitOutcome::Committed(completed)) => Ok(completed),
            // Held by a stronger in-flight registration (it may be restored
            // if the winner fails). To this caller the tx is not committed
            // *now*, so surface the race outcome as a rejection; the hold
            // machinery decides the rest.
            Ok(super::submit::VerifySubmitOutcome::Superseded) => Err(Reject::RBFRejected(
                super::TxPoolService::SUPERSEDED_BY_HIGHER_FEE_CANDIDATE.to_string(),
            )),
            Ok(super::submit::VerifySubmitOutcome::Cleared) => Err(Self::stale_pipeline_reject()),
            Err(reject) => Err(reject),
        }
    }

    /// Check if a transaction depends on any in-flight pipeline transaction.
    /// If so, route it to the ordered resolve queue.
    async fn check_and_route_dependent(
        &self,
        tx: &TransactionView,
        source: TxSource,
        epoch: u64,
    ) -> Result<RouteDecision, Reject> {
        let id = tx.proposal_short_id();

        if !self.is_pipeline_epoch_current(epoch) {
            return Err(Self::stale_pipeline_reject());
        }

        if self.depends_on_pipeline(tx).await {
            let mut ordered = self.pipeline.queues.ordered_resolve_queue.write().await;
            if !self.is_pipeline_epoch_current(epoch) {
                return Err(Self::stale_pipeline_reject());
            }
            if ordered.contains_key(&id) {
                return Ok(RouteDecision::Duplicate);
            }
            return ordered
                .add_tx(crate::resolved_tx::ResolveJob::new_at(
                    tx.clone(),
                    source,
                    epoch,
                ))
                .map(|_| RouteDecision::Enqueued);
        }

        Ok(RouteDecision::Independent)
    }

    /// Enqueue a resolved transaction into the verify queue, applying the
    /// in-flight RBF fee-ordering gate first for remote replacements.
    ///
    /// This is the single entry into the verify queue: both the entry
    /// classifier and the ordered resolver go through here, so the RBF gate
    /// cannot be bypassed.
    ///
    /// For RBF replacements, the candidate is validated and the displacement
    /// set computed while holding `rbf_candidates.write()`, then inserted into
    /// the verify queue and the registration committed atomically. This
    /// guarantees that lower-fee-rate displaced candidates are only removed
    /// from the pipeline once the higher-fee-rate candidate is successfully
    /// queued (P0-2 fix), and maintains the global lock order
    /// `rbf_candidates → verify_queue` (P0-1 fix). Only remote txs register:
    /// local and proposal txs skip the in-flight fee-rate gate.
    pub(crate) async fn enqueue_resolved_tx(
        &self,
        resolved: crate::resolved_tx::ResolvedTx,
    ) -> Result<bool, Reject> {
        let source = resolved.source;
        let tx = resolved.tx.clone();
        let epoch = resolved.epoch;

        self.ensure_current_or_terminal(&tx, source, epoch).await?;

        if matches!(source, TxSource::Remote { .. }) {
            match self
                .register_rbf_candidate(
                    tx.clone(),
                    source,
                    &resolved,
                    resolved.fee,
                    resolved.tx_size,
                )
                .await
            {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(reject) => {
                    if !self.is_pipeline_epoch_current(epoch) {
                        self.terminal_internal(tx, source).await;
                    }
                    return Err(reject);
                }
            }
            if !self.is_pipeline_epoch_current(epoch) {
                return self
                    .ensure_current_or_terminal(&tx, source, epoch)
                    .await
                    .map(|_| false);
            }
        }

        // Release the verify_queue lock before running after_process:
        // after_process may acquire tx_pool and other locks, and must never
        // run while holding a pipeline-queue write lock.
        let add_result = {
            let mut verify_queue = self.pipeline.queues.verify_queue.write().await;
            if self.is_pipeline_epoch_current(epoch) {
                verify_queue.add_tx(resolved)
            } else {
                Err(Self::stale_pipeline_reject())
            }
        };
        match add_result {
            Ok(added) => Ok(added),
            Err(reject) => {
                if self.is_pipeline_epoch_current(epoch) {
                    self.after_process_at(tx, source, &Err(reject.clone()), epoch)
                        .await;
                } else {
                    self.terminal_internal(tx, source).await;
                }
                Err(reject)
            }
        }
    }

    /// Classify a transaction and enqueue it for verification or ordered resolve.
    ///
    /// This is the core entry-point classifier.  It checks whether the tx
    /// depends on an in-flight pipeline tx, runs the shared resolve step, and
    /// routes the result to the appropriate queue.
    pub(crate) async fn classify_and_enqueue_tx(
        &self,
        tx: TransactionView,
        source: TxSource,
    ) -> Result<bool, Reject> {
        let epoch = self.current_pipeline_epoch()?;
        self.classify_and_enqueue_tx_at(tx, source, epoch).await
    }

    pub(crate) async fn classify_and_enqueue_tx_at(
        &self,
        tx: TransactionView,
        source: TxSource,
        epoch: u64,
    ) -> Result<bool, Reject> {
        let id = tx.proposal_short_id();

        self.ensure_current_or_terminal(&tx, source, epoch).await?;

        let route = self.check_and_route_dependent(&tx, source, epoch).await;
        let route = match route {
            Ok(route) => route,
            Err(reject) if !self.is_pipeline_epoch_current(epoch) => {
                self.terminal_internal(tx, source).await;
                return Err(reject);
            }
            Err(reject) => return Err(reject),
        };
        match route {
            RouteDecision::Independent => {}
            RouteDecision::Enqueued => return Ok(true),
            RouteDecision::Duplicate => return Ok(false),
        }

        // The resolve step is shared with the ordered resolver so the
        // pre_check logic exists in exactly one place.
        match crate::resolve_mgr::resolve_job(
            self,
            crate::resolved_tx::ResolveJob::new_at(tx.clone(), source, epoch),
        )
        .await
        {
            crate::resolve_mgr::ResolveStageResult::Ready(resolved) => {
                self.enqueue_resolved_tx(resolved).await
            }
            crate::resolve_mgr::ResolveStageResult::Orphan(..) => {
                // Missing inputs: park the tx in the ordered resolve queue so
                // the ordered resolver retries it once its parents land.
                let mut ordered = self.pipeline.queues.ordered_resolve_queue.write().await;
                if !self.is_pipeline_epoch_current(epoch) {
                    drop(ordered);
                    self.terminal_internal(tx, source).await;
                    return Err(Self::stale_pipeline_reject());
                }
                if ordered.contains_key(&id) {
                    return Ok(false);
                }
                ordered.add_tx(crate::resolved_tx::ResolveJob::new_at(tx, source, epoch))
            }
            crate::resolve_mgr::ResolveStageResult::Reject(tx, reject) => {
                self.after_process_at(tx, source, &Err(reject.clone()), epoch)
                    .await;
                Err(reject)
            }
        }
    }

    /// Entry-point classifier used by remote/local submission and proposal
    /// notifications (`notify_tx`).
    ///
    /// Dependent transactions (those that spend an output currently in flight)
    /// are handled synchronously so they land in the ordered resolve queue in
    /// arrival order and errors propagate to the caller.  Independent
    /// transactions are sent to a fixed-size worker pool so that the expensive
    /// `pre_check` work does not serialize inside the service actor.
    pub(crate) async fn classify_and_enqueue_tx_spawn(
        &self,
        tx: TransactionView,
        source: TxSource,
    ) -> Result<bool, Reject> {
        let epoch = self.current_pipeline_epoch()?;
        self.ensure_current_or_terminal(&tx, source, epoch).await?;
        let route = self.check_and_route_dependent(&tx, source, epoch).await;
        let route = match route {
            Ok(route) => route,
            Err(reject) if !self.is_pipeline_epoch_current(epoch) => {
                self.terminal_internal(tx, source).await;
                return Err(reject);
            }
            Err(reject) => return Err(reject),
        };
        match route {
            RouteDecision::Independent => {}
            RouteDecision::Enqueued => return Ok(true),
            RouteDecision::Duplicate => return Ok(false),
        }

        let job = PreCheckJob { tx, source, epoch };
        if let Err(reject) = self
            .pipeline
            .queues
            .pre_check_queue
            .push_if_current(job.clone(), &self.pipeline.epoch)
        {
            if !self.is_pipeline_epoch_current(epoch) {
                self.terminal_internal(job.tx, job.source).await;
            }
            return Err(reject);
        }

        // Returning Ok(true) only means the tx was accepted into the pipeline;
        // actual classification/verification happens in the worker pool.
        Ok(true)
    }
}
