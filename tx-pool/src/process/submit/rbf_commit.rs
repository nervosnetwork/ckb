//! The write-locked commit transaction family: from RBF conflict checking
//! to pool insertion.
//!
//! This module carries the write-lock transaction steps that run after
//! `verify_and_submit_core`:
//! `prepare_rbf_replacement` (conflict check + removal + progressive
//! export), `try_submit_entry` (the write-lock boundary + failure
//! recovery), `commit_and_apply_limits` / `commit_entry_to_pool` (pool
//! insertion and size limits), and `dispatch_submit_aftermath`
//! (out-of-lock side-effect dispatch). Entry and verification orchestration
//! lives in `super` (`process/submit/mod.rs`).

use crate::callback::Callbacks;
use crate::component::entry::TxEntry;
use crate::component::pool_map::Status;
use crate::error::Reject;
use crate::pool::TxPool;
use crate::pool::rbf::RbfCheck;
use crate::service::TxPoolService;
use crate::tx_source::TxSource;
use crate::util::time_relative_verify;
use ckb_logger::{debug, warn};
use ckb_snapshot::Snapshot;
use ckb_types::core::error::OutPointError;
use ckb_types::core::{TransactionView, cell::ResolvedTransaction};
use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use std::collections::HashSet;
use std::sync::Arc;

use crate::process::{get_tx_status, status_to_verify_env};

/// Txs recovered from the conflicts cache after a failed or successful
/// replacement, paired with the pipeline source they were recorded with.
pub(crate) type RecoveredTxs = Vec<(TransactionView, TxSource)>;

/// Outcome of `try_submit_entry`: the commit result, whether an actual RBF
/// replacement happened (old transactions were removed), the recovered txs
/// to re-enqueue, and the reject events to dispatch outside the write lock.
pub(crate) type SubmitEntryOutcome = (
    Result<(), Reject>,
    bool,
    RecoveredTxs,
    Vec<(TxEntry, Reject)>,
);

/// The side effects accumulated by one submit attempt.
///
/// Everything here is exported *progressively* by `prepare_rbf_replacement`
/// and `commit_and_apply_limits`, so every failure path already holds the
/// full record: `recover_on_failure` merges the conflict-cache recovery,
/// the escape-hatch evictions, and the conflict removal into one consistent
/// recovery set and suppresses the spurious reject events (see the call
/// sites for the invariants).
#[derive(Default)]
pub(crate) struct SubmitSideEffects {
    /// Reject events to dispatch outside the write lock.
    reject_events: Vec<(TxEntry, Reject)>,
    /// Pool entries removed by `process_rbf` (direct conflicts + cascade).
    removed_old_txs: Vec<TxEntry>,
    /// Txs to recover (conflict-cache hits and escape-hatch evictions).
    recovered: RecoveredTxs,
    /// Entries evicted by `add_entry`'s cell-ref escape hatch during this
    /// attempt (drained from `PoolMap::evicted_journal`).
    commit_evicted: HashSet<TxEntry>,
}

impl SubmitSideEffects {
    /// Merge every recovery source and suppress spurious events after a
    /// failed submit. Must be called with the pool write guard held, while
    /// the conflicts cache still owns the recovered txs.
    fn recover_on_failure(
        &mut self,
        tx_pool: &mut TxPool,
        entry: &TxEntry,
        entry_id: &ProposalShortId,
    ) {
        // If the replacement was rejected after `process_rbf` removed the
        // old conflicting transactions, the new tx's inputs are free
        // again. Recover any txs stored in the waiting room for those
        // inputs (in particular the old tx itself) so that a failed
        // RBF attempt cannot be used to evict in-pool transactions.
        //
        // IMPORTANT: exclude the entry transaction itself from recovery.
        // It was just rejected by RBF; re-enqueueing it would cause a
        // cycle where both the entry and the in-pool tx keep being
        // recovered and failing RBF against each other indefinitely.
        //
        // `recovered` may already hold entries that `prepare_rbf_replacement`
        // exported before the failing step; dedup by short id when extending
        // from the entry's own inputs.
        let mut seen: HashSet<ProposalShortId> = self
            .recovered
            .iter()
            .map(|(tx, _)| tx.proposal_short_id())
            .collect();
        self.recovered.extend(
            tx_pool
                .get_conflicted_txs_from_inputs(entry.transaction().input_pts_iter())
                .into_iter()
                .filter(|(tx, _)| {
                    tx.proposal_short_id() != *entry_id && seen.insert(tx.proposal_short_id())
                }),
        );

        // Entries evicted by `add_entry`'s cell-ref escape hatch while
        // this commit was being attempted (an over-limit ancestry was
        // trimmed by evicting in-pool txs that cell-dep on the new tx's
        // inputs). With the commit rejected, the invalidation never
        // happened, so they must be recovered too — this class of
        // eviction was previously lost with only an `Invalidated`
        // reject notification.
        let existing: HashSet<ProposalShortId> = self
            .recovered
            .iter()
            .map(|(tx, _)| tx.proposal_short_id())
            .collect();
        self.recovered.extend(
            std::mem::take(&mut self.commit_evicted)
                .into_iter()
                .filter_map(|evicted| {
                    let tx = evicted.transaction().clone();
                    // Same source policy as conflict-cache recovery: the
                    // original pipeline source is not retained in the pool.
                    (!existing.contains(&tx.proposal_short_id())
                        && tx.proposal_short_id() != *entry_id)
                        .then_some((tx, TxSource::Local))
                }),
        );

        // Recovery order matters: parents must be re-added before
        // children, while escape-hatch evictions arrive children-first
        // (post-order removal). Re-sort the merged set by dependency.
        TxPoolService::sort_by_dependencies(&mut self.recovered, |(tx, _)| tx);

        for (tx, _) in &self.recovered {
            tx_pool.remove_conflict(&tx.proposal_short_id());
        }
        for old in &self.removed_old_txs {
            tx_pool.remove_conflict(&old.proposal_short_id());
        }

        // When RBF fails, the old transactions removed by process_rbf are
        // being recovered back into the pool.  Suppress their reject
        // callbacks to avoid spurious reject-then-accept sequences: the
        // subscriber would first hear "tx X was replaced" and then see X
        // reappear as pending.
        let recovered_ids: HashSet<ProposalShortId> = self
            .recovered
            .iter()
            .map(|(tx, _)| tx.proposal_short_id())
            .chain(self.removed_old_txs.iter().map(|e| e.proposal_short_id()))
            .collect();
        self.reject_events
            .retain(|(entry, _)| !recovered_ids.contains(&entry.proposal_short_id()));
    }
}

type AddToPoolFn = fn(&mut TxPool, TxEntry) -> Result<(bool, HashSet<TxEntry>), Reject>;
type PoolCallbackFn = fn(&Callbacks, &TxEntry);

impl TxPoolService {
    /// Check RBF conflicts, re-verify if the tip changed while the tx was in
    /// flight, remove old conflicting transactions, and collect transactions that
    /// can be recovered from the waiting room (conflict recovery).
    ///
    /// All work happens inside the write-lock transaction boundary so that any
    /// error rolls back the `TxPool` mutations.
    ///
    /// `fx` is filled *progressively*, right after the infallible steps that
    /// produce each part and before the fallible tip-change revalidation: on
    /// every error path the caller already holds the full removal record and
    /// the partial recovery set, so its recovery and reject-suppression logic
    /// behaves identically no matter which step failed.
    pub(crate) fn prepare_rbf_replacement(
        &self,
        tx_pool: &mut TxPool,
        snapshot: &Arc<Snapshot>,
        pre_resolve_tip: Byte32,
        entry: &TxEntry,
        mut status: Status,
        fx: &mut SubmitSideEffects,
    ) -> Result<Status, Reject> {
        // check_rbf must be invoked in `write` lock to avoid concurrent issues.
        // It returns the direct conflicts plus their shared conflict closure
        // (post-ordered removal plan + membership set), computed in one
        // traversal.
        let RbfCheck {
            conflicts,
            removal,
            removal_set,
        } = if tx_pool.enable_rbf() {
            tx_pool.check_rbf(snapshot, entry)?
        } else {
            // RBF is disabled but we found conflicts, return error here
            // after_process will put this tx into conflicts_pool
            let conflicted_outpoint = tx_pool.pool_map.find_conflict_outpoint(entry.transaction());
            if let Some(outpoint) = conflicted_outpoint {
                return Err(Reject::Resolve(OutPointError::Dead(outpoint)));
            }
            RbfCheck {
                conflicts: HashSet::new(),
                removal: Vec::new(),
                removal_set: HashSet::new(),
            }
        };

        // Pre-validate that committing the entry can actually succeed
        // *before* removing the conflicts it replaces. `process_rbf`
        // removes the conflicts and their descendants; if the entry is
        // certain to fail the ancestor-count limit even after that
        // removal, the removal would only churn the pool —
        // evict-then-restore the whole conflict cluster on every attempt —
        // for a replacement that never had a chance to commit (a failed
        // replacement pays no fee, so the churn is free to repeat).
        // Rejecting here leaves the pool untouched; borderline cases still
        // fall through to the normal remove-and-recover path.
        if !conflicts.is_empty() {
            tx_pool
                .pool_map
                .validate_ancestor_capacity(entry.transaction(), &removal_set)?;
        }

        // Remove conflicting transactions *before* re-checking the resolved
        // transaction. `check_rtx` uses `PoolCell` in non-RBF mode, so any
        // input still consumed by an in-pool conflict would be reported as
        // `Dead`. Removing the conflicts first keeps a tip change from
        // incorrectly rejecting a valid RBF replacement.
        //
        // The removed set is exported immediately: every fallible step after
        // this point must leave the caller holding the full removal record,
        // otherwise its error path cannot recover the cascade (descendants
        // whose inputs differ from the replacement's would be stranded in
        // the conflicts cache) or suppress their "replaced" reject events.
        fx.removed_old_txs = tx_pool.process_rbf(entry, &removal, &mut fx.reject_events);

        // Txs whose inputs are not consumed by the new tx can be
        // recovered immediately, regardless of whether the new tx
        // ultimately succeeds. This computation is infallible, so it runs
        // before the tip-change revalidation for the same reason as above:
        // the caller's recovery set must be complete on every error path.
        let mut available_inputs: HashSet<OutPoint> = HashSet::new();
        available_inputs.extend(
            fx.removed_old_txs
                .iter()
                .flat_map(|removed| removed.transaction().input_pts_iter()),
        );
        for input in entry.transaction().input_pts_iter() {
            available_inputs.remove(&input);
        }
        fx.recovered
            .extend(tx_pool.get_conflicted_txs_from_inputs(available_inputs.into_iter()));

        // Parents must be recovered before children so that the
        // ordered resolver can re-resolve and accept them in the
        // correct order.
        Self::sort_by_dependencies(&mut fx.recovered, |(tx, _)| tx);

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

        Ok(status)
    }
    /// Commit the entry to the pool, record reject events for evicted txs, and
    /// apply size limits.
    pub(crate) fn commit_and_apply_limits(
        &self,
        tx_pool: &mut TxPool,
        entry: &TxEntry,
        status: Status,
        fx: &mut SubmitSideEffects,
    ) -> Result<(), Reject> {
        let evicted = commit_entry_to_pool(
            tx_pool,
            status,
            entry,
            &self.relay.callbacks,
            &mut fx.commit_evicted,
        )?;

        // `commit_entry_to_pool` has already drained `PoolMap::evicted_journal`
        // (the cell-ref escape-hatch evictions) into `fx.commit_evicted`, and
        // on success it equals the returned evict set — no second merge here.
        // On the `Err` path that journal is the only channel carrying the
        // evicted entries (see `SubmitSideEffects::recover_on_failure`).

        // in a corner case, a tx with lower fee rate may be rejected immediately
        // after inserting into pool, return proper reject error here
        for evict in evicted {
            let reject =
                Reject::Invalidated(format!("invalidated by tx {}", evict.transaction().hash()));
            fx.reject_events.push((evict, reject));
        }

        tx_pool.remove_conflict(&entry.proposal_short_id());
        tx_pool
            .limit_size(Some(&entry.proposal_short_id()), &mut fx.reject_events)
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
        let mut fx = SubmitSideEffects::default();

        // The closure is the write-lock transaction boundary: any error rolls
        // back the `TxPool` mutations made inside it.
        let result = (|| -> Result<(), Reject> {
            let final_status = self.prepare_rbf_replacement(
                tx_pool,
                &snapshot,
                pre_resolve_tip,
                &entry,
                status,
                &mut fx,
            )?;

            self.commit_and_apply_limits(tx_pool, &entry, final_status, &mut fx)?;

            Ok(())
        })();

        // Whether this commit actually replaced in-pool transactions. The
        // aftermath uses it to choose finalize (really reject the held
        // losers) vs abort (restore them): a successful submit that removed
        // nothing — its conflicts were evicted by a third party before the
        // commit — replaced no one, so rejecting its held candidates would
        // be wrong.
        let replaced = !fx.removed_old_txs.is_empty();
        if result.is_err() {
            fx.recover_on_failure(tx_pool, &entry, &entry_id);
        }
        // Note: on success the recovered txs stay in the conflict cache
        // while they are re-enqueued — the cache is their durable home
        // until they reach a terminal state (re-committed or terminally
        // rejected). Pulling them out here would make the whole removed
        // cluster vanish from the conflicts view the moment the
        // replacement lands, and the transient double registration (queue
        // slot + cache entry) is benign: re-enqueue dedups by id, and
        // `remove_tx` clears both sides.

        (result, replaced, fx.recovered, fx.reject_events)
    }
    /// Dispatch reject callbacks, enqueue recovered txs, clean up stale RBF
    /// registrations, and remove the current RBF candidate after the write-locked
    /// submit is complete.
    pub(crate) async fn dispatch_submit_aftermath(
        &self,
        entry_id: &ProposalShortId,
        result: Result<(), Reject>,
        replaced: bool,
        recovered: RecoveredTxs,
        reject_events: Vec<(TxEntry, Reject)>,
    ) -> Result<(), Reject> {
        // Dispatch reject callbacks outside the write lock, regardless of
        // whether the submission itself succeeded.
        for (entry, reject) in &reject_events {
            self.relay.callbacks.call_reject(entry, reject.clone());
        }

        // In-pool entries that were removed by this submit (RBF-replaced or
        // evicted by size limits) have freed their inputs. Clean up any RBF
        // candidates still targeting those inputs so they do not block future
        // replacements.
        self.cleanup_rbf_for_removed_entries(reject_events.iter().map(|(entry, _)| entry))
            .await;

        // Send recovered txs to the deferred worker after the write lock is
        // released. Use .send().await rather than try_send so that recovery
        // txs are never silently dropped under high RBF frequency.
        // Recovery is attempted even if the replacement ultimately failed,
        // because the old conflicting txs have already been removed from the
        // pool and may now be valid again.
        if !recovered.is_empty()
            && let Err(e) = self
                .pipeline
                .deferred_sender
                .send(crate::service::DeferredTask::RecoverTxs(recovered))
                .await
        {
            warn!("failed to enqueue recovered txs for re-processing: {}", e);
        }

        // The RBF candidate has either been accepted or definitively
        // rejected; remove it from the in-flight fee-ordering gate. On
        // success *with an actual replacement* the candidates it displaced
        // are really rejected (finalize); on failure — or on a success that
        // removed nothing because the conflicts had already vanished — they
        // are restored to the verify queue (abort). Finalizing without an
        // actual replacement would wrongly reject losers whose inputs are
        // free again (see the hold-and-restore contract in `rbf_candidates`).
        if result.is_ok() && replaced {
            self.finalize_rbf_candidate(entry_id).await;
        } else {
            self.abort_rbf_candidate(entry_id).await;
        }

        result
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
    evicted_out: &mut HashSet<TxEntry>,
) -> Result<HashSet<TxEntry>, Reject> {
    let tx_hash = entry.transaction().hash();
    debug!("submit_entry {:?} {}", status, tx_hash);
    let (add, callback): (AddToPoolFn, PoolCallbackFn) = match status {
        Status::Pending => (TxPool::add_pending, Callbacks::call_pending),
        Status::Gap => (TxPool::add_gap, Callbacks::call_pending),
        Status::Proposed => (TxPool::add_proposed, Callbacks::call_proposed),
    };
    let result = add(tx_pool, entry.clone());
    // Drain the escape-hatch journal into the caller's recovery set on
    // *both* outcomes: on success it equals the returned evict set (the
    // `HashSet` dedups), on failure it is the only channel that still
    // carries the evicted entries (see `PoolMap::evicted_journal`).
    evicted_out.extend(std::mem::take(&mut tx_pool.pool_map.evicted_journal));
    let (succ, evicts) = result?;
    if !succ {
        // `add` returns `succ == false` when the entry's short-id slot is
        // already occupied. The pipeline-wide duplicate checks (classify
        // scans every queue, `check_txid_collision` scans the pool, and each
        // queue dedups internally) make this unreachable today. If it ever
        // does fire, the conflicts removed by `process_rbf` above must be
        // recovered through the normal `Err` path instead of being left
        // evicted while this entry is silently dropped. `Duplicated` is the
        // one reject exempt from recent-reject recording, so surfacing it
        // does not punish a later legitimate resubmission.
        return Err(Reject::Duplicated(tx_hash));
    }
    callback(callbacks, entry);
    Ok(evicts)
}
