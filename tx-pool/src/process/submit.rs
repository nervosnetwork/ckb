use crate::callback::Callbacks;
use crate::component::entry::TxEntry;
use crate::component::pipeline_queue::PipelineQueue;
use crate::component::pool_map::Status;
use crate::error::Reject;
use crate::pool::TxPool;
use crate::service::TxPoolService;
use crate::try_or_return_with_snapshot;
use crate::util::{time_relative_verify, verify_rtx};
use ckb_logger::{debug, info, warn};
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_types::core::error::OutPointError;
use ckb_types::core::{TransactionView, cell::ResolvedTransaction};
use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use ckb_verification::cache::{CacheEntry, Completed};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::watch;

use super::{
    PreCheckedTx, SubmitEntryOutcome, VerifyAndSubmitInput, get_tx_status, status_to_verify_env,
};
use crate::tx_source::TxSource;

type AddToPoolFn = fn(&mut TxPool, TxEntry) -> Result<(bool, HashSet<TxEntry>), Reject>;
type PoolCallbackFn = fn(&Callbacks, &TxEntry);

impl TxPoolService {
    pub(crate) async fn fetch_tx_verify_cache(&self, tx: &TransactionView) -> Option<CacheEntry> {
        let guard = self.txs_verify_cache.read().await;
        guard.peek(&tx.witness_hash()).cloned()
    }
    /// Check RBF conflicts, re-verify if the tip changed while the tx was in
    /// flight, remove old conflicting transactions, and collect transactions that
    /// can be recovered from the conflict pool.
    ///
    /// All work happens inside the write-lock transaction boundary so that any
    /// error rolls back the `TxPool` mutations.
    pub(crate) fn prepare_rbf_replacement(
        &self,
        tx_pool: &mut TxPool,
        snapshot: &Arc<Snapshot>,
        pre_resolve_tip: Byte32,
        entry: &TxEntry,
        mut status: Status,
        reject_events: &mut Vec<(TxEntry, Reject)>,
    ) -> Result<(Vec<TxEntry>, Vec<TransactionView>, Status), Reject> {
        // check_rbf must be invoked in `write` lock to avoid concurrent issues.
        let conflicts = if tx_pool.enable_rbf() {
            tx_pool.check_rbf(snapshot, entry)?
        } else {
            // RBF is disabled but we found conflicts, return error here
            // after_process will put this tx into conflicts_pool
            let conflicted_outpoint = tx_pool.pool_map.find_conflict_outpoint(entry.transaction());
            if let Some(outpoint) = conflicted_outpoint {
                return Err(Reject::Resolve(OutPointError::Dead(outpoint)));
            }
            HashSet::new()
        };

        // if snapshot changed by context switch we need redo time_relative verify
        let tip_hash = snapshot.tip_hash();
        if pre_resolve_tip != tip_hash {
            debug!(
                "submit_entry {} context changed. previous:{} now:{}",
                entry.proposal_short_id(),
                pre_resolve_tip,
                tip_hash
            );

            status = check_rtx(tx_pool, snapshot, &entry.rtx)?;

            let tip_header = snapshot.tip_header();
            let tx_env = status_to_verify_env(status, tip_header);
            time_relative_verify(Arc::clone(snapshot), Arc::clone(&entry.rtx), tx_env)?;
        }

        let removed_old_txs = self.process_rbf(tx_pool, entry, &conflicts, reject_events);

        // Txs whose inputs are not consumed by the new tx can be
        // recovered immediately, regardless of whether the new tx
        // ultimately succeeds.
        let mut available_inputs: HashSet<OutPoint> = HashSet::new();
        available_inputs.extend(
            removed_old_txs
                .iter()
                .flat_map(|removed| removed.transaction().input_pts_iter()),
        );
        for input in entry.transaction().input_pts_iter() {
            available_inputs.remove(&input);
        }
        let mut recovered = Vec::new();
        recovered.extend(tx_pool.get_conflicted_txs_from_inputs(available_inputs.into_iter()));

        // Parents must be recovered before children so that the
        // ordered resolver can re-resolve and accept them in the
        // correct order.
        Self::sort_txs_by_dependencies(&mut recovered);

        Ok((removed_old_txs, recovered, status))
    }
    /// Commit the entry to the pool, record reject events for evicted txs, and
    /// apply size limits.
    pub(crate) fn commit_and_apply_limits(
        &self,
        tx_pool: &mut TxPool,
        entry: &TxEntry,
        status: Status,
        reject_events: &mut Vec<(TxEntry, Reject)>,
    ) -> Result<(), Reject> {
        let evicted = commit_entry_to_pool(tx_pool, status, entry, &self.callbacks)?;

        // in a corner case, a tx with lower fee rate may be rejected immediately
        // after inserting into pool, return proper reject error here
        for evict in evicted {
            let reject =
                Reject::Invalidated(format!("invalidated by tx {}", evict.transaction().hash()));
            reject_events.push((evict, reject));
        }

        tx_pool.remove_conflict(&entry.proposal_short_id());
        tx_pool
            .limit_size(Some(&entry.proposal_short_id()), reject_events)
            .map_or(Ok(()), Err)
    }
    pub(crate) fn try_submit_entry(
        &self,
        tx_pool: &mut TxPool,
        snapshot: Arc<Snapshot>,
        pre_resolve_tip: Byte32,
        entry: TxEntry,
        status: Status,
        entry_id: ProposalShortId,
    ) -> SubmitEntryOutcome {
        let mut reject_events = Vec::new();
        let mut recovered = Vec::new();
        let mut removed_old_txs = Vec::new();

        // The closure is the write-lock transaction boundary: any error rolls
        // back the `TxPool` mutations made inside it.
        let result = (|| -> Result<(), Reject> {
            let (removed, rec, final_status) = self.prepare_rbf_replacement(
                tx_pool,
                &snapshot,
                pre_resolve_tip,
                &entry,
                status,
                &mut reject_events,
            )?;
            removed_old_txs = removed;
            recovered = rec;

            self.commit_and_apply_limits(tx_pool, &entry, final_status, &mut reject_events)?;

            Ok(())
        })();

        // If the replacement was rejected after `process_rbf` removed the
        // old conflicting transactions, the new tx's inputs are free
        // again. Recover any txs stored in the conflict pool for those
        // inputs (in particular the old tx itself) so that a failed
        // RBF attempt cannot be used to evict in-pool transactions.
        //
        // IMPORTANT: exclude the entry transaction itself from recovery.
        // It was just rejected by RBF; re-enqueueing it would cause a
        // cycle where both the entry and the in-pool tx keep being
        // recovered and failing RBF against each other indefinitely.
        if result.is_err() {
            recovered.extend(
                tx_pool
                    .get_conflicted_txs_from_inputs(entry.transaction().input_pts_iter())
                    .into_iter()
                    .filter(|tx| tx.proposal_short_id() != entry_id),
            );
            for tx in &recovered {
                tx_pool.remove_conflict(&tx.proposal_short_id());
            }
            for old in &removed_old_txs {
                tx_pool.remove_conflict(&old.proposal_short_id());
            }

            // When RBF fails, the old transactions removed by process_rbf are
            // being recovered back into the pool.  Suppress their reject
            // callbacks to avoid spurious reject-then-accept sequences: the
            // subscriber would first hear "tx X was replaced" and then see X
            // reappear as pending.
            let recovered_ids: HashSet<ProposalShortId> = recovered
                .iter()
                .map(|tx| tx.proposal_short_id())
                .chain(removed_old_txs.iter().map(|e| e.proposal_short_id()))
                .collect();
            reject_events.retain(|(entry, _)| !recovered_ids.contains(&entry.proposal_short_id()));
        }

        (result, recovered, reject_events)
    }
    /// Dispatch reject callbacks, enqueue recovered txs, clean up stale RBF
    /// registrations, and remove the current RBF candidate after the write-locked
    /// submit is complete.
    async fn dispatch_submit_aftermath(
        &self,
        entry_id: &ProposalShortId,
        result: Result<(), Reject>,
        recovered: Vec<TransactionView>,
        reject_events: Vec<(TxEntry, Reject)>,
        snapshot: Arc<Snapshot>,
    ) -> (Result<(), Reject>, Arc<Snapshot>) {
        // Dispatch reject callbacks outside the write lock, regardless of
        // whether the submission itself succeeded.
        for (entry, reject) in &reject_events {
            self.callbacks.call_reject(entry, reject.clone());
        }

        // In-pool entries that were removed by this submit (RBF-replaced or
        // evicted by size limits) have freed their inputs. Clean up any RBF
        // candidates still targeting those inputs so they do not block future
        // replacements.
        self.cleanup_rbf_for_removed_entries(&reject_events).await;

        // Send recovered txs to the deferred worker after the write lock is
        // released. Use .send().await rather than try_send so that recovery
        // txs are never silently dropped under high RBF frequency.
        // Recovery is attempted even if the replacement ultimately failed,
        // because the old conflicting txs have already been removed from the
        // pool and may now be valid again.
        if !recovered.is_empty()
            && let Err(e) = self
                .deferred_sender
                .send(crate::service::DeferredTask::RecoverTxs(recovered))
                .await
        {
            warn!("failed to enqueue recovered txs for re-processing: {}", e);
        }

        // The RBF candidate has either been accepted or definitively rejected;
        // remove it from the in-flight fee-ordering gate.
        self.remove_rbf_candidate(entry_id).await;

        (result, snapshot)
    }

    pub(crate) async fn submit_entry(
        &self,
        pre_resolve_tip: Byte32,
        entry: TxEntry,
        status: Status,
    ) -> (Result<(), Reject>, Arc<Snapshot>) {
        let conflict_inputs = self.find_conflict_inputs(entry.transaction()).await;
        let entry_id = entry.proposal_short_id();

        if !conflict_inputs.is_empty() {
            // Hold the RBF read lock across the tx_pool write so that no
            // higher-fee-rate candidate can register between the superseded
            // check and the actual submit. The lock ordering here is rbf
            // (read) -> tx_pool (write), consistent with the intended
            // pipeline hierarchy.
            let entry_fee_rate = entry.fee_rate();
            let rbf_guard = self.rbf_candidates.read().await;
            if rbf_guard.is_superseded(&entry_id, entry_fee_rate, &conflict_inputs) {
                drop(rbf_guard);
                self.remove_rbf_candidate(&entry_id).await;
                let (_, snapshot) = self
                    .read_tx_pool_with_snapshot(|_tx_pool, snapshot| snapshot)
                    .await;
                return (
                    Err(Reject::RBFRejected(
                        "superseded by higher-fee-rate in-flight candidate".to_string(),
                    )),
                    snapshot,
                );
            }

            let mut tx_pool = self.tx_pool.write().await;
            let snapshot = tx_pool.cloned_snapshot();
            let (result, recovered, reject_events) = self.try_submit_entry(
                &mut tx_pool,
                Arc::clone(&snapshot),
                pre_resolve_tip,
                entry,
                status,
                entry_id.clone(),
            );
            drop(tx_pool);
            drop(rbf_guard);

            self.dispatch_submit_aftermath(&entry_id, result, recovered, reject_events, snapshot)
                .await
        } else {
            // Separate the successful result from the collected reject events and
            // recovered txs. Reject callbacks must be dispatched and displaced txs
            // must be recovered even if the closure returns an error after
            // `process_rbf` has already removed old transactions (e.g. the
            // replacement fails the pool ancestor/size limits). Without this, a
            // remote peer can evict in-pool txs via a crafted RBF replacement that
            // is itself rejected, leaving the node with neither transaction.
            let ((result, recovered, reject_events), snapshot) = self
                .with_tx_pool_write_lock(|tx_pool, snapshot| {
                    self.try_submit_entry(
                        tx_pool,
                        snapshot,
                        pre_resolve_tip.clone(),
                        entry.clone(),
                        status,
                        entry_id.clone(),
                    )
                })
                .await;

            self.dispatch_submit_aftermath(&entry_id, result, recovered, reject_events, snapshot)
                .await
        }
    }
    pub(crate) async fn test_accept_tx(&self, tx: TransactionView) -> Result<Completed, Reject> {
        self.check_tx_basic_validity(&tx, TxSource::local()).await?;
        self.test_accept_tx_core(tx.clone()).await
    }
    /// Verify and submit a transaction whose inputs have already been resolved.
    ///
    /// This is the second stage of the tx-pool pipeline: the resolver has
    /// already produced a [`ResolvedTx`], and this function runs the CPU-heavy
    /// contextual verification and the final write-locked submit.
    pub(crate) async fn verify_and_submit_tx(
        &self,
        resolved: crate::resolved_tx::ResolvedTx,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Option<(Result<Completed, Reject>, Arc<Snapshot>)> {
        let crate::resolved_tx::ResolvedTx {
            tx,
            rtx,
            status,
            fee,
            tx_size,
            pre_resolve_tip,
            snapshot,
            source,
        } = resolved;

        self.verify_and_submit_core(
            VerifyAndSubmitInput {
                tx,
                rtx,
                status,
                fee,
                tx_size,
                pre_resolve_tip,
                snapshot,
                source,
            },
            command_rx,
        )
        .await
    }
    /// Side effects run after a transaction has been successfully submitted to
    /// the pool: notify the block assembler, wake the ordered resolver, enqueue
    /// a verify cache update, and record metrics.
    pub(crate) async fn post_submit_side_effects(
        &self,
        status: Status,
        verified: Completed,
        verify_cache: Option<CacheEntry>,
        wtx_hash: &Byte32,
        is_sync_process: bool,
        instant: Instant,
    ) {
        self.notify_block_assembler(status).await;

        // A newly submitted transaction may resolve dependent transactions that
        // are waiting in the ordered resolve queue (e.g. children of a parent
        // that was just re-added after a reorg). Wake the ordered resolver so
        // those children can be retried promptly.
        let queue = self.queues.ordered_resolve_queue.read().await;
        if !queue.is_empty() {
            queue.subscribe().notify_one();
        }

        if verify_cache.is_none() {
            // Defer cache update to the background worker instead of
            // spawning a fire-and-forget task.
            if let Err(e) =
                self.deferred_sender
                    .try_send(crate::service::DeferredTask::CacheUpdate {
                        wtx_hash: wtx_hash.clone(),
                        verified,
                    })
            {
                warn!(
                    "failed to enqueue verify cache update for {}: {}",
                    wtx_hash, e
                );
            }
        }

        if let Some(metrics) = ckb_metrics::handle() {
            let elapsed = instant.elapsed().as_secs_f64();
            if is_sync_process {
                metrics.ckb_tx_pool_sync_process.observe(elapsed);
            } else {
                metrics.ckb_tx_pool_async_process.observe(elapsed);
            }
        }
    }
    /// Shared core: verify a resolved transaction and submit it to the pool.
    ///
    /// Both `process_tx_direct` (reorg recovery / local RPC path) and
    /// `verify_and_submit_tx` (pipeline verify path) converge here after the
    /// resolve step.
    pub(crate) async fn verify_and_submit_core(
        &self,
        input: VerifyAndSubmitInput,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Option<(Result<Completed, Reject>, Arc<Snapshot>)> {
        let VerifyAndSubmitInput {
            tx,
            rtx,
            status,
            fee,
            tx_size,
            pre_resolve_tip,
            snapshot,
            source,
        } = input;
        let declared_cycles = source.cycles();
        // Verification uses the snapshot captured at resolve time. If the chain
        // tip has advanced since then (detected via pre_resolve_tip != tip_hash),
        // prepare_rbf_replacement re-runs check_rtx + time_relative_verify against
        // the current snapshot to catch any state-dependent invalidation.
        let wtx_hash = tx.witness_hash();
        let instant = Instant::now();
        let is_sync_process = command_rx.is_none();

        let verify_cache = self.fetch_tx_verify_cache(&tx).await;
        let max_cycles = declared_cycles.unwrap_or_else(|| self.consensus.max_block_cycles());
        let tip_header = snapshot.tip_header();
        let tx_env = Arc::new(status_to_verify_env(status, tip_header));

        let verified_ret = verify_rtx(
            Arc::clone(&snapshot),
            Arc::clone(&rtx),
            tx_env,
            &verify_cache,
            max_cycles,
            command_rx,
        )
        .await;

        let verified = match verified_ret {
            Ok(v) => v,
            Err(err) => {
                self.remove_rbf_candidate(&tx.proposal_short_id()).await;
                return Some((Err(err), snapshot));
            }
        };

        if let Some(declared) = declared_cycles
            && declared != verified.cycles
        {
            info!(
                "declared cycles not match verified cycles, declared: {}, verified: {}, tx_hash: {}",
                declared,
                verified.cycles,
                tx.hash()
            );
            self.remove_rbf_candidate(&tx.proposal_short_id()).await;
            return Some((
                Err(Reject::DeclaredWrongCycles(declared, verified.cycles)),
                snapshot,
            ));
        }

        let entry = TxEntry::new(rtx, verified.cycles, fee, tx_size);

        let (ret, submit_snapshot) = self.submit_entry(pre_resolve_tip, entry, status).await;
        try_or_return_with_snapshot!(ret, submit_snapshot);

        self.post_submit_side_effects(
            status,
            verified,
            verify_cache,
            &wtx_hash,
            is_sync_process,
            instant,
        )
        .await;

        Some((Ok(verified), submit_snapshot))
    }
    pub(crate) async fn test_accept_tx_core(
        &self,
        tx: TransactionView,
    ) -> Result<Completed, Reject> {
        let tx_size = tx.data().serialized_size_in_block();
        let (pre_check_ret, snapshot) = self.pre_check(&tx, tx_size).await;

        let PreCheckedTx { rtx, status, .. } = pre_check_ret?;

        // skip check the delay window

        let verify_cache = self.fetch_tx_verify_cache(&tx).await;
        let max_cycles = self.consensus.max_block_cycles();
        let tip_header = snapshot.tip_header();
        let tx_env = Arc::new(status_to_verify_env(status, tip_header));

        verify_rtx(
            Arc::clone(&snapshot),
            Arc::clone(&rtx),
            tx_env,
            &verify_cache,
            max_cycles,
            None,
        )
        .await
    }
}

fn check_rtx(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    rtx: &ResolvedTransaction,
) -> Result<Status, Reject> {
    let short_id = rtx.transaction.proposal_short_id();
    let tx_status = get_tx_status(snapshot, &short_id);
    tx_pool.check_rtx_from_pool(rtx).map(|_| tx_status)
}

fn commit_entry_to_pool(
    tx_pool: &mut TxPool,
    status: Status,
    entry: &TxEntry,
    callbacks: &Callbacks,
) -> Result<HashSet<TxEntry>, Reject> {
    let tx_hash = entry.transaction().hash();
    debug!("submit_entry {:?} {}", status, tx_hash);
    let (add, callback): (AddToPoolFn, PoolCallbackFn) = match status {
        Status::Pending => (TxPool::add_pending, Callbacks::call_pending),
        Status::Gap => (TxPool::add_gap, Callbacks::call_pending),
        Status::Proposed => (TxPool::add_proposed, Callbacks::call_proposed),
    };
    let (succ, evicts) = add(tx_pool, entry.clone())?;
    if succ {
        callback(callbacks, entry);
    }
    Ok(evicts)
}
