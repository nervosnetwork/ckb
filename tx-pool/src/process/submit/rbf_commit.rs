//! The write-locked commit transaction family: from RBF conflict checking
//! to pool insertion.
//!
//! This module carries the write-lock transaction steps that run after
//! `verify_and_submit_core`:
//! `prepare_rbf_replacement` (conflict check + removal + progressive
//! export), `try_submit_entry` (the write-lock boundary + failure
//! recovery), `commit_and_apply_limits` / `commit_entry_to_pool` (pool
//! insertion and size limits), and synchronous effect journaling. Entry and
//! verification orchestration lives in `super` (`process/submit/mod.rs`).

use crate::component::entry::TxEntry;
use crate::component::pool_map::{PoolMapAddOutcome, RemovedPoolEntry, Status};
use crate::error::Reject;
use crate::pool::TxPool;
use crate::pool::rbf::RbfCheck;
use crate::service::TxPoolService;
use crate::service::effects::TxPoolEffect;
use crate::util::time_relative_verify;
use ckb_logger::debug;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::core::error::OutPointError;
use ckb_types::core::{TransactionView, cell::ResolvedTransaction};
use ckb_types::packed::{Byte32, ProposalShortId};
use std::collections::HashSet;
use std::sync::Arc;

use crate::process::{get_tx_status, status_to_verify_env};

#[cfg(test)]
#[path = "../tests/rbf_commit_seam.rs"]
mod test_seam;

/// Entries already restored inside the authoritative pool write transaction,
/// retained only for diagnostics and regression assertions.
pub(crate) type RolledBackTxs = Vec<(TransactionView, Status)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemovalCause {
    Replacement,
    AncestorEscape,
    SizeLimit,
}

#[derive(Debug, Clone)]
struct JournaledRemoval {
    removed: RemovedPoolEntry,
    cause: RemovalCause,
}

/// One authoritative record of every physical removal made by a commit.
/// Failure rollback consumes all causes; successful effect generation can
/// select the cause it needs without maintaining parallel partial journals.
#[derive(Debug, Default)]
struct PoolCommitJournal {
    removals: Vec<JournaledRemoval>,
}

impl PoolCommitJournal {
    fn record(&mut self, cause: RemovalCause, removed: impl IntoIterator<Item = RemovedPoolEntry>) {
        self.removals.extend(
            removed
                .into_iter()
                .map(|removed| JournaledRemoval { removed, cause }),
        );
    }

    fn contains(&self, cause: RemovalCause) -> bool {
        self.removals.iter().any(|item| item.cause == cause)
    }

    fn by_cause(&self, cause: RemovalCause) -> impl Iterator<Item = &RemovedPoolEntry> + '_ {
        self.removals
            .iter()
            .filter(move |item| item.cause == cause)
            .map(|item| &item.removed)
    }

    fn rollback_entries(&self, rejected_entry: &ProposalShortId) -> Vec<RemovedPoolEntry> {
        let mut seen = HashSet::new();
        self.removals
            .iter()
            .filter_map(|item| {
                let id = item.removed.entry.proposal_short_id();
                (id != *rejected_entry && seen.insert(id)).then(|| item.removed.clone())
            })
            .collect()
    }
}

/// Outcome of `try_submit_entry`, carried as one side-effect envelope across
/// the tx-pool write-lock boundary.
pub(crate) struct SubmitEntryOutcome {
    pub(crate) result: Result<(), Reject>,
    /// Whether an actual RBF replacement happened (old transactions were
    /// physically removed).
    pub(crate) replaced: bool,
    /// Entries restored before `try_submit_entry` returned and before the pool
    /// write guard can be released.
    pub(crate) rolled_back: RolledBackTxs,
    /// Terminal removals whose reject callbacks run outside the lock.
    pub(crate) reject_events: Vec<(TxEntry, Reject)>,
    /// Successful accepted callback, also dispatched outside the lock.
    pub(crate) accept_event: Option<(TxEntry, Status)>,
}

/// Submit result that retains the exact pool mutation journal until the
/// pre-pool kernel has finalized the matching Ready ticket. The journal is
/// private to this module so callers cannot publish or partially replay it.
pub(crate) struct CoordinatedSubmitOutcome {
    pub(crate) outcome: SubmitEntryOutcome,
    pool_journal: PoolCommitJournal,
    /// Historical Wait publication is deliberately later than the tentative
    /// pool insertion. It becomes true only after the matching pre-pool
    /// handoff has succeeded under the same TxPool write guard.
    history_finalized: bool,
}

impl CoordinatedSubmitOutcome {
    /// Accepted transaction hashes physically removed by this successful
    /// commit whose outputs are also absent from the active snapshot. This is
    /// the exact dependency-unavailability set handed to the pre-pool kernel
    /// before the pool write guard is released.
    pub(crate) fn unavailable_parent_hashes(&self, snapshot: &Snapshot) -> HashSet<Byte32> {
        if self.outcome.result.is_err() {
            return HashSet::new();
        }
        self.pool_journal
            .removals
            .iter()
            .map(|item| item.removed.entry.transaction().hash())
            // A startup-reload window can temporarily retain an entry whose
            // raw transaction is already committed. Removing that overlay
            // owner does not make its outputs unavailable on the active
            // snapshot and must not demote pre-pool consumers.
            .filter(|hash| !snapshot.transaction_exists(hash))
            .collect()
    }

    /// Minimal assembler refresh set for this committed pool transaction.
    /// A replacement can remove a Proposed entry while inserting a Pending
    /// one, so the new entry alone is not a complete template delta.
    pub(crate) fn block_assembler_statuses(&self) -> HashSet<Status> {
        if self.outcome.result.is_err() {
            return HashSet::new();
        }
        let mut statuses = self
            .outcome
            .accept_event
            .iter()
            .map(|(_, status)| *status)
            .collect::<HashSet<_>>();
        statuses.extend(
            self.pool_journal
                .removals
                .iter()
                .map(|item| item.removed.status),
        );
        statuses
    }
}

/// The side effects accumulated by one submit attempt.
///
/// Everything here is journaled *progressively* by `prepare_rbf_replacement`
/// and `commit_and_apply_limits`, so every failure path already holds the
/// full record: `rollback_on_failure` restores every journaled physical
/// removal under the pool lock and suppresses spurious reject events.
#[derive(Default)]
pub(crate) struct SubmitSideEffects {
    /// Reject events to dispatch outside the write lock.
    reject_events: Vec<(TxEntry, Reject)>,
    /// Every physical pool removal made by this submit, classified by cause.
    pool_journal: PoolCommitJournal,
    /// Entries restored inside the pool write transaction after a failed
    /// commit. Exported only as diagnostic evidence; aftermath never
    /// re-processes them through the pipeline.
    rolled_back: RolledBackTxs,
    /// Successful pool insertion notification, dispatched only after the
    /// write lock is released. It is intentionally installed after all pool
    /// limits pass so a transaction rejected by the same submit never emits
    /// a spurious pending/proposed callback first.
    accept_event: Option<(TxEntry, Status)>,
}

impl SubmitSideEffects {
    /// Merge every recovery source and suppress spurious events after a
    /// failed submit. Must be called with the pool write guard held, before
    /// any tentative removal is published as historical conflict Wait.
    fn rollback_on_failure(
        &mut self,
        tx_pool: &mut TxPool,
        entry_id: &ProposalShortId,
    ) -> Result<(), Reject> {
        // Build one exact physical-removal journal. Besides RBF conflicts and
        // escape-hatch evictions this includes unrelated low-fee entries that
        // `limit_size` may have removed before it rejected the candidate.
        // Those entries were previously omitted from rollback entirely.
        let rollback_entries = self.pool_journal.rollback_entries(entry_id);
        // Restore before returning to the caller, while it still owns the
        // same `TxPool` write guard. The PoolMap primitive owns parent-first
        // ordering and rejects any duplicate/eviction during reconstruction.
        // Because these exact entries formed a valid prior pool state and the
        // rejected candidate is absent, insertion is logically infallible.
        let restored = tx_pool
            .pool_map
            .restore_removed_entries_exact(rollback_entries)?;
        for (tx, status) in restored {
            self.rolled_back.push((tx, status));
        }

        // When RBF fails, the old transactions removed by process_rbf are
        // being recovered back into the pool.  Suppress their reject
        // callbacks to avoid spurious reject-then-accept sequences: the
        // subscriber would first hear "tx X was replaced" and then see X
        // reappear as pending.
        let recovered_ids: HashSet<ProposalShortId> = self
            .rolled_back
            .iter()
            .map(|(tx, _)| tx.proposal_short_id())
            .collect();
        self.reject_events
            .retain(|(entry, _)| !recovered_ids.contains(&entry.proposal_short_id()));
        Ok(())
    }
}

type AddToPoolFn = fn(&mut TxPool, TxEntry) -> Result<PoolMapAddOutcome, Reject>;

impl TxPoolService {
    /// Check RBF conflicts, re-verify if the tip changed while the tx was in
    /// flight, and journal every removed transaction for exact rollback and
    /// later bounded conflict discovery.
    ///
    /// All work happens inside the write-lock transaction boundary so that any
    /// error rolls back the `TxPool` mutations.
    ///
    /// `fx` is filled *progressively*, right after the infallible steps that
    /// produce each part and before the fallible tip-change revalidation: on
    /// every error path the caller already holds the full physical-removal
    /// record needed to restore the pre-submit state.
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
                .validate_ancestor_capacity(entry, &removal_set)?;
        }

        // Remove conflicting transactions *before* re-checking the resolved
        // transaction. `check_rtx` uses `PoolCell` in non-RBF mode, so any
        // input still consumed by an in-pool conflict would be reported as
        // `Dead`. Removing the conflicts first keeps a tip change from
        // incorrectly rejecting a valid RBF replacement.
        //
        // The removed set is exported immediately: every fallible step after
        // this point must leave the caller holding the full removal record,
        // otherwise its error path cannot restore the cascade or suppress its
        // spurious "replaced" reject events.
        let removed = if removal.is_empty() {
            Vec::new()
        } else {
            tx_pool.process_rbf(entry, &removal, &mut fx.reject_events)
        };
        fx.pool_journal.record(RemovalCause::Replacement, removed);

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
        let evicted = commit_entry_to_pool(tx_pool, status, entry, &mut fx.pool_journal)?;

        // `commit_entry_to_pool` moves the successful cell-ref escape-hatch
        // cohort into `fx.pool_journal`. PoolMap itself restores that cohort
        // before any insertion error is allowed to cross this boundary.

        // in a corner case, a tx with lower fee rate may be rejected immediately
        // after inserting into pool, return proper reject error here
        for evict in evicted {
            let reject =
                Reject::Invalidated(format!("invalidated by tx {}", evict.transaction().hash()));
            fx.reject_events.push((evict, reject));
        }

        let mut removed = Vec::new();
        let reject = tx_pool.limit_size_with_journal(
            Some(&entry.proposal_short_id()),
            &mut fx.reject_events,
            &mut removed,
        );
        fx.pool_journal.record(RemovalCause::SizeLimit, removed);
        reject.map_or(Ok(()), Err)
    }
    fn try_submit_entry_inner(
        &self,
        tx_pool: &mut TxPool,
        snapshot: Arc<Snapshot>,
        pre_resolve_tip: Byte32,
        entry: TxEntry,
        status: Status,
        entry_id: ProposalShortId,
    ) -> CoordinatedSubmitOutcome {
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
            fx.accept_event = Some((entry.clone(), final_status));

            Ok(())
        })();

        // Whether this commit actually replaced in-pool transactions. The
        // aftermath uses it to choose finalize (really reject the held
        // losers) vs abort (restore them): a successful submit that removed
        // nothing — its conflicts were evicted by a third party before the
        // commit — replaced no one, so rejecting its held candidates would
        // be wrong.
        let replaced = fx.pool_journal.contains(RemovalCause::Replacement);
        if result.is_err()
            && let Err(rollback_error) = fx.rollback_on_failure(tx_pool, &entry_id)
        {
            self.pipeline.kernel.fail_stop(
                "tx-pool exact rollback failed after rejected submit",
                &(result, rollback_error),
            );
        }

        CoordinatedSubmitOutcome {
            outcome: SubmitEntryOutcome {
                result,
                replaced,
                rolled_back: fx.rolled_back,
                reject_events: fx.reject_events,
                accept_event: fx.accept_event,
            },
            pool_journal: fx.pool_journal,
            history_finalized: false,
        }
    }

    /// Publish historical ownership only after the tentative PoolMap mutation
    /// and its matching coordinator handoff have both succeeded. This is the
    /// state-side counterpart of the effect outbox linearization point: a
    /// failed cross-authority handoff can still restore the exact prior pool
    /// without evicting unrelated bounded history or deleting the candidate's
    /// existing cached copy.
    pub(crate) fn finalize_coordinated_submit(
        &self,
        tx_pool: &mut TxPool,
        entry: &TxEntry,
        coordinated: &mut CoordinatedSubmitOutcome,
    ) {
        assert!(
            coordinated.outcome.result.is_ok(),
            "only a successful pool/coordinator handoff can publish conflict history"
        );
        assert!(
            !coordinated.history_finalized,
            "conflict history is finalized exactly once per submit"
        );

        // Register every outpoint that became usable as a dependency: inputs
        // released by RBF/ancestor/size removals, plus outputs created by the
        // newly accepted entry. The latter cascades historical parent -> child
        // recovery without scanning under the pool lock.
        //
        let mut available_outpoints = tx_pool.released_inputs_from_removed_entries(
            coordinated
                .pool_journal
                .removals
                .iter()
                .map(|item| &item.removed.entry),
        );
        available_outpoints.extend(entry.transaction().output_pts());
        let available_dependencies =
            crate::service::pipeline_ops::available_cell_dependencies(tx_pool, available_outpoints);
        let epoch = self.pipeline.epoch.current().unwrap_or(0);
        self.pipeline.kernel.mutate_required(
            "pool commit conflict-history settlement failed",
            |kernel| {
                kernel.remove_conflict_hash(&entry.transaction().hash());
                // Advance availability first. Newly retained victims observe
                // this exact epoch and cannot immediately wake on their own
                // removal; a later level change starts the next pass.
                kernel.note_available(available_dependencies)?;
                for victim in coordinated.pool_journal.by_cause(RemovalCause::Replacement) {
                    let tx = victim.entry.transaction().clone();
                    let keys = crate::component::pre_pool::conflict_dependency_keys(
                        &tx,
                        victim.entry.related_dep_out_points().cloned(),
                    );
                    let raw = crate::component::pre_pool::PipelineRawTx::new(
                        tx,
                        crate::tx_source::TxSource::Local,
                        epoch,
                    );
                    let owner = crate::component::pre_pool::historical_source(
                        crate::tx_source::TxSource::Local,
                    );
                    if let Err(error) = kernel.retain_conflict(
                        raw,
                        owner,
                        keys,
                        crate::component::pre_pool::historical_deadline(owner),
                    ) {
                        ckb_logger::warn!(
                            "dropping bounded RBF history after pre-pool backpressure: {error:?}"
                        );
                    }
                }
                Ok(())
            },
        );
        coordinated.history_finalized = true;
    }

    pub(crate) fn try_submit_entry_coordinated(
        &self,
        tx_pool: &mut TxPool,
        snapshot: Arc<Snapshot>,
        pre_resolve_tip: Byte32,
        entry: TxEntry,
        status: Status,
        entry_id: ProposalShortId,
    ) -> CoordinatedSubmitOutcome {
        self.try_submit_entry_inner(tx_pool, snapshot, pre_resolve_tip, entry, status, entry_id)
    }

    /// Undo a successful pool commit when coordinator finalization could not
    /// complete. This runs under the same `TxPool` write guard, before any
    /// effect is published, and restores every RBF/escape/size-limit removal
    /// from the retained exact journal.
    pub(crate) fn rollback_coordinated_submit(
        &self,
        tx_pool: &mut TxPool,
        entry: &TxEntry,
        coordinated: &mut CoordinatedSubmitOutcome,
        cause: Reject,
    ) -> Result<(), Reject> {
        if coordinated.outcome.result.is_err() {
            return Ok(());
        }
        if coordinated.history_finalized {
            return Err(Reject::Malformed(
                "pool".to_string(),
                "coordinator rollback attempted after conflict history finalization".to_string(),
            ));
        }
        let entry_id = entry.proposal_short_id();
        let removed = tx_pool.pool_map.remove_entry_with_status(&entry_id);
        let Some(removed) = removed else {
            return Err(Reject::Malformed(
                "pool".to_string(),
                format!(
                    "coordinator rollback could not find newly committed transaction {}",
                    entry.transaction().hash()
                ),
            ));
        };
        if removed.entry.transaction().hash() != entry.transaction().hash() {
            return Err(Reject::Malformed(
                "pool".to_string(),
                "coordinator rollback removed a different short-id owner".to_string(),
            ));
        }

        let mut effects = SubmitSideEffects {
            reject_events: std::mem::take(&mut coordinated.outcome.reject_events),
            pool_journal: std::mem::take(&mut coordinated.pool_journal),
            rolled_back: std::mem::take(&mut coordinated.outcome.rolled_back),
            accept_event: None,
        };
        effects.rollback_on_failure(tx_pool, &entry_id)?;
        coordinated.outcome.result = Err(cause);
        coordinated.outcome.replaced = false;
        coordinated.outcome.rolled_back = effects.rolled_back;
        coordinated.outcome.reject_events = effects.reject_events;
        coordinated.outcome.accept_event = None;
        Ok(())
    }
    /// Bind the complete stable-state publication batch while the caller still
    /// holds the authoritative TxPool write lock. This is the effect
    /// linearization point: concurrent commits cannot publish in a different
    /// order from their pool mutations, and cancellation after unlock cannot
    /// lose the already-journaled result.
    pub(crate) fn journal_submit_effects(
        &self,
        outcome: &mut SubmitEntryOutcome,
        permit: crate::service::effects::EffectPermit,
        mut extra_effects: Vec<TxPoolEffect>,
    ) {
        if outcome.result.is_ok() {
            debug_assert!(
                outcome.rolled_back.is_empty(),
                "successful submit must not export rolled-back entries"
            );
        }

        let mut effects = Vec::new();
        if let Some((entry, status)) = outcome.accept_event.take()
            && let Some(effect) = self.accepted_effect(entry, status)
        {
            effects.push(effect);
        }
        for (entry, reject) in std::mem::take(&mut outcome.reject_events) {
            effects.extend(self.rejected_effects(entry, reject));
        }
        effects.append(&mut extra_effects);
        self.publish_required_reserved_effects(
            permit,
            effects,
            "reserved submit effect journal failed inside pool transaction",
        );
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
    journal: &mut PoolCommitJournal,
) -> Result<Vec<TxEntry>, Reject> {
    #[cfg(test)]
    if std::mem::take(&mut tx_pool.fail_next_pool_commit_panic) {
        panic!("injected authoritative pool commit panic");
    }
    let tx_hash = entry.transaction().hash();
    debug!("submit_entry {:?} {}", status, tx_hash);
    let add: AddToPoolFn = match status {
        Status::Pending => TxPool::add_pending,
        Status::Gap => TxPool::add_gap,
        Status::Proposed => TxPool::add_proposed,
    };
    let PoolMapAddOutcome { inserted, evicted } = add(tx_pool, entry.clone())?;
    if !inserted {
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
    let evicted_entries = evicted
        .iter()
        .map(|removed| removed.entry.clone())
        .collect();
    journal.record(RemovalCause::AncestorEscape, evicted);
    Ok(evicted_entries)
}
