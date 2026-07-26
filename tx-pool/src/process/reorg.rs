use crate::component::entry::TxEntry;
use crate::component::pool_map::{ConflictClosure, RemovedPoolEntry, Status};
use crate::error::Reject;
use crate::pool::TxPool;
use ckb_logger::debug;
use ckb_snapshot::Snapshot;
use ckb_types::core::TransactionView;
use ckb_types::packed::{Byte32, ProposalShortId};
use ckb_util::LinkedHashSet;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Retain detached transactions whose raw transaction hash is absent from
/// the attached branch. `TransactionView` equality includes witnesses, but
/// cell identity and spendability use the raw transaction hash: treating two
/// witness variants as distinct would re-submit the detached variant as Dead
/// and incorrectly cascade that failure into otherwise-live dependents.
pub(crate) fn detached_not_attached(
    detached: &LinkedHashSet<TransactionView>,
    attached: &LinkedHashSet<TransactionView>,
) -> Vec<TransactionView> {
    let attached_hashes: HashSet<Byte32> = attached.iter().map(TransactionView::hash).collect();
    detached
        .iter()
        .filter(|tx| !attached_hashes.contains(&tx.hash()))
        .cloned()
        .collect()
}

/// Collected results of [`update_tx_pool_for_reorg`]. The service binds their
/// pre-pool membership delta and effect journal before releasing the pool
/// write lock; external callbacks run later through the effect publisher.
#[derive(Default)]
pub(crate) struct ReorgOutcome {
    /// Reject events for removed entries.
    pub(crate) reject_events: Vec<(TxEntry, Reject)>,
    /// Entries removed silently by the post-startup reconcile: they freed
    /// their inputs, so the RBF-registration cleanup must see them too.
    pub(crate) silently_removed: Vec<TxEntry>,
    /// Accepted entries transferred atomically to `RecoveryRetained`. Their
    /// causal producers are temporarily unavailable, but their inputs are not
    /// published as released to conflict history while replay is pending.
    pub(crate) recovery_removed: Vec<TxEntry>,
    /// Proposed/pending notifications (user callbacks must not run
    /// in-lock).
    pub(crate) notify_events: Vec<(TxEntry, Status)>,
}

/// Exact accepted-pool ownership that must move back through authoritative
/// recovery because one of its causal producers was detached from the chain.
///
/// The removal order is child-first, matching `PoolMap`'s total removal
/// primitive. Recovery payloads are exposed in the reverse (parent-first)
/// order so ordinary admission never needs a special late-parent mutation.
pub(crate) enum AcceptedRecoveryPlan {
    Bounded { removals: Vec<PlannedRecoveryEntry> },
    OverBound,
}

pub(crate) struct PlannedRecoveryEntry {
    id: ProposalShortId,
    hash: Byte32,
    status: Status,
    entry: TxEntry,
}

impl AcceptedRecoveryPlan {
    pub(crate) fn transactions_parent_first(&self) -> Vec<TransactionView> {
        match self {
            Self::Bounded { removals } => removals
                .iter()
                .rev()
                .map(|planned| planned.entry.transaction().clone())
                .collect(),
            Self::OverBound => Vec::new(),
        }
    }
}

/// Plan the accepted causal closure made parentless by detached transactions.
///
/// A transaction already in the accepted pool can legally depend on a
/// committed chain transaction. If that producer is later detached, leaving
/// the consumer resident while replaying the producer would require a second
/// late-parent graph mutation (and would expose an unresolvable accepted
/// entry during replay). Instead, move the complete bounded closure back to
/// `RecoveryRetained` and replay the existing ordinary parent-first path.
pub(crate) fn plan_accepted_recovery(
    tx_pool: &TxPool,
    detached: &[TransactionView],
    limit: usize,
) -> AcceptedRecoveryPlan {
    let mut roots = HashSet::new();
    for tx in detached {
        for output in tx.output_pts() {
            if let Some(spender) = tx_pool.pool_map.out_point_index.get_input_ref(&output) {
                roots.insert(spender.clone());
            }
            if let Some(readers) = tx_pool.pool_map.out_point_index.get_deps_ref(&output) {
                roots.extend(readers.iter().cloned());
            }
        }
    }

    let ConflictClosure::Complete { removal, .. } =
        tx_pool.pool_map.conflict_closure(&roots, limit)
    else {
        return AcceptedRecoveryPlan::OverBound;
    };
    let removals = removal
        .into_iter()
        .filter_map(|id| {
            tx_pool
                .pool_map
                .get_by_id(&id)
                .map(|current| PlannedRecoveryEntry {
                    id,
                    hash: current.hash.clone(),
                    status: current.status,
                    entry: current.inner.clone(),
                })
        })
        .collect();
    AcceptedRecoveryPlan::Bounded { removals }
}

/// Total Apply for a previously validated accepted-recovery plan. The caller
/// holds the pool write guard continuously between Plan and Apply.
pub(crate) fn apply_accepted_recovery(
    tx_pool: &mut TxPool,
    plan: AcceptedRecoveryPlan,
) -> Vec<RemovedPoolEntry> {
    let AcceptedRecoveryPlan::Bounded { removals } = plan else {
        unreachable!("an over-bound accepted recovery uses generation reset")
    };
    removals
        .into_iter()
        .map(|planned| {
            let current = tx_pool
                .pool_map
                .get_by_id(&planned.id)
                .expect("planned reorg recovery entry remains accepted");
            assert_eq!(current.hash, planned.hash);
            assert_eq!(current.status, planned.status);
            tx_pool
                .pool_map
                .remove_entry_with_status(&planned.id)
                .expect("planned reorg recovery removal is total")
        })
        .collect()
}

/// Begin the accepted-pool half of one reorg transaction. Startup zombie
/// reconciliation and size limiting are deliberately deferred: the caller
/// must first transfer descendants of detached producers to retained recovery,
/// otherwise the startup sweep would destroy the very closure that reorg is
/// required to replay.
pub(crate) fn begin_tx_pool_reorg(
    tx_pool: &mut TxPool,
    attached: &LinkedHashSet<TransactionView>,
    detached_headers: &HashSet<Byte32>,
    detached_proposal_id: HashSet<ProposalShortId>,
    snapshot: Arc<Snapshot>,
    mine_mode: bool,
) -> Result<ReorgOutcome, Reject> {
    let mut reject_events = Vec::new();
    // Proposed/pending notifications are *collected* and dispatched by the
    // caller outside the write lock: running user callbacks while holding
    // the tx-pool write lock would let a blocking callback stall (or
    // re-enter and deadlock) the whole pool.
    let mut notify_events = Vec::new();

    // No error may escape after snapshot, membership, index, or status
    // mutation begins. Eventual replay convergence is not enough: exposing a
    // partial slice before the retained reorg retries violates the accepted
    // pool/coordinator ownership boundary and has no matching effect journal.
    tx_pool.preflight_reorg_status_transitions()?;
    tx_pool.snapshot = Arc::clone(&snapshot);

    // Demote detached proposals and their causal descendants in place. For a transaction
    // which is both expired and committed at the one time(commit at its end of commit-window),
    // we should treat it as a committed and not re-put into pending-pool. So we should ensure
    // that involves `remove_committed_txs` before `remove_expired`.
    tx_pool.remove_committed_txs(attached.iter(), detached_headers, &mut reject_events);
    tx_pool.remove_by_detached_proposal(detached_proposal_id.iter(), &mut notify_events);

    // Re-evaluate Gap/Pending against the new tip's proposal windows.
    //
    // Demotion runs for every node (not only mine mode): a Gap entry whose
    // short id has left both the gap and proposed sets is simply wrong —
    // RPC still reports "pending" (Gap maps to Pending) while
    // `get_proposals` / `TxSelector` never touch it, and verify env also
    // treats it as proposed. Promotion stays mine-mode only (packaging).
    //
    // Transitions (collected first so index iteration is stable):
    //   Gap     + proposed → Proposed   (mine mode)
    //   Gap     + gap      → stay Gap
    //   Gap     + neither  → Pending    (always; notify)
    //   Pending + proposed → Proposed   (mine mode)
    //   Pending + gap      → Gap        (mine mode)
    let mut to_proposed = Vec::new();
    let mut to_gap = Vec::new();
    let mut to_pending = Vec::new();

    for entry in tx_pool.pool_map.entries.get_by_status(&Status::Gap) {
        let short_id = entry.inner.proposal_short_id();
        if snapshot.proposals().contains_proposed(&short_id) {
            if mine_mode {
                to_proposed.push((short_id, entry.inner.clone()));
            }
        } else if !snapshot.proposals().contains_gap(&short_id) {
            to_pending.push((short_id, entry.inner.clone()));
        }
    }

    if mine_mode {
        for entry in tx_pool.pool_map.entries.get_by_status(&Status::Pending) {
            let short_id = entry.inner.proposal_short_id();
            let elem = (short_id.clone(), entry.inner.clone());
            if snapshot.proposals().contains_proposed(&short_id) {
                to_proposed.push(elem);
            } else if snapshot.proposals().contains_gap(&short_id) {
                to_gap.push(elem);
            }
        }
    }

    for (id, entry) in to_proposed {
        debug!("begin to proposed: {:x}", id);
        tx_pool.transition_to_status_required(&id, Status::Proposed);
        notify_events.push((entry, Status::Proposed));
    }

    for (id, _) in to_gap {
        debug!("begin to gap: {:x}", id);
        tx_pool.transition_to_status_required(&id, Status::Gap);
    }

    for (id, entry) in to_pending {
        debug!("begin to demote gap to pending: {:x}", id);
        tx_pool.transition_to_status_required(&id, Status::Pending);
        // Re-pending: block assembler must re-select this short id for
        // proposals (Gap is invisible to `get_proposals`).
        notify_events.push((entry, Status::Pending));
    }

    // Remove expired transaction from pending
    tx_pool.remove_expired(&mut reject_events);

    Ok(ReorgOutcome {
        reject_events,
        silently_removed: Vec::new(),
        recovery_removed: Vec::new(),
        notify_events,
    })
}

/// Complete the accepted reorg transaction after the bounded recovery
/// ownership transfer. This tail is total and runs under the same pool write
/// guard as [`begin_tx_pool_reorg`].
pub(crate) fn finish_tx_pool_reorg(tx_pool: &mut TxPool, outcome: &mut ReorgOutcome) {
    // One-shot post-startup reconcile for entries committed (or zombied)
    // while their reorg notifications were skipped during the startup
    // reload. This runs against the fresh snapshot swapped in above, so the
    // first reorg after startup cleans up that window; afterwards it is
    // skipped — a full scan with a store lookup per entry is too expensive
    // to repeat on every block. The removed entries are returned to the
    // caller: they freed their inputs, so the same RBF-registration
    // cleanup that runs for the reject events must also run for them
    // (otherwise ghost registrations block future replacements forever).
    if !tx_pool.onchain_reconcile_done {
        tx_pool.onchain_reconcile_done = true;
        outcome.silently_removed = tx_pool.remove_onchain_entries();
        if !outcome.silently_removed.is_empty() {
            debug!(
                "reconcile dropped {} on-chain pool entries",
                outcome.silently_removed.len()
            );
        }
    }

    // Remove transactions from the pool until its size <= size_limit.
    let current_reject = tx_pool.limit_size(None, &mut outcome.reject_events);
    debug_assert!(
        current_reject.is_none(),
        "reorg size reconciliation has no distinguished incoming entry"
    );

    // Notifications are published only after this whole pool mutation. Do
    // not export intermediate Pending -> Proposed steps, or a Pending event
    // for an entry that expiry/size reconciliation removed later in the same
    // transaction. Coalesce by full hash and read the final authoritative
    // status; a short-id collision must never attribute another transaction.
    let rejected_hashes: HashSet<_> = outcome
        .reject_events
        .iter()
        .map(|(entry, _)| entry.transaction().hash())
        .collect();
    let mut positions = HashMap::new();
    let mut stable_notify_events: Vec<(TxEntry, Status)> = Vec::new();
    for (entry, _) in std::mem::take(&mut outcome.notify_events) {
        let hash = entry.transaction().hash();
        if rejected_hashes.contains(&hash) {
            continue;
        }
        let Some(current) = tx_pool.pool_map.get_by_hash(&hash) else {
            continue;
        };
        let final_event = (current.inner.clone(), current.status);
        if let Some(index) = positions.get(&hash).copied() {
            stable_notify_events[index] = final_event;
        } else {
            positions.insert(hash, stable_notify_events.len());
            stable_notify_events.push(final_event);
        }
    }

    outcome.notify_events = stable_notify_events;
}

/// Standalone compatibility wrapper used by focused pool tests. Production
/// reorg orchestration calls Begin, performs the cross-authority recovery
/// transfer, then calls Finish under one uninterrupted write guard.
#[cfg(test)]
pub(crate) fn update_tx_pool_for_reorg(
    tx_pool: &mut TxPool,
    attached: &LinkedHashSet<TransactionView>,
    detached_headers: &HashSet<Byte32>,
    detached_proposal_id: HashSet<ProposalShortId>,
    snapshot: Arc<Snapshot>,
    mine_mode: bool,
) -> Result<ReorgOutcome, Reject> {
    let mut outcome = begin_tx_pool_reorg(
        tx_pool,
        attached,
        detached_headers,
        detached_proposal_id,
        snapshot,
        mine_mode,
    )?;
    finish_tx_pool_reorg(tx_pool, &mut outcome);
    Ok(outcome)
}

#[cfg(test)]
#[path = "tests/reorg.rs"]
mod tests;
