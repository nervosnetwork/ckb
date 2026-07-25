use crate::component::entry::TxEntry;
use crate::component::pool_map::Status;
use crate::error::Reject;
use crate::pool::TxPool;
use crate::service::effects::{EffectBatch, EffectClass};
use crate::util::compact_packed;
use ckb_logger::debug;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::core::TransactionView;
use ckb_types::packed::{Byte32, ProposalShortId};
use ckb_types::prelude::Entity;
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
pub(crate) struct ReorgOutcome {
    /// Reject events for removed entries.
    pub(crate) reject_events: Vec<(TxEntry, Reject)>,
    /// Entries removed silently by the post-startup reconcile: they freed
    /// their inputs, so the RBF-registration cleanup must see them too.
    pub(crate) silently_removed: Vec<TxEntry>,
    /// Proposed/pending notifications (user callbacks must not run
    /// in-lock).
    pub(crate) notify_events: Vec<(TxEntry, Status)>,
}

impl crate::service::TxPoolService {
    /// A detached transaction that cannot be recovered makes every accepted
    /// input/cell-dep consumer of its outputs permanently unresolvable. Remove
    /// the complete descendant closure while holding the universal TxPool ->
    /// pre-pool boundary, then bind callbacks and Wait availability changes
    /// before the pool mutation becomes visible.
    pub(crate) async fn cascade_failed_reorg_recovery(&self, tx: &TransactionView) {
        let effect_bound = self.max_reorg_effect_bytes();
        if self
            .relay
            .effects
            .wait_capacity(effect_bound, EffectClass::Critical)
            .await
            .is_err()
        {
            return;
        }
        let mut tx_pool = self.pool.tx_pool.write().await;
        self.pipeline.kernel.guard_authoritative_mutation(
            "reorg recovery cascade mutation panicked",
            || {
                let result = self.pipeline.kernel.mutate(|kernel| {
                    self.relay.effects.try_apply_bounded(
                        effect_bound,
                        EffectClass::Critical,
                        || {
                let mut roots: HashMap<ProposalShortId, ckb_types::packed::OutPoint> =
                    HashMap::new();
                for out_point in tx.output_pts() {
                    if let Some(id) = tx_pool
                        .pool_map
                        .out_point_index
                        .get_input_ref(&out_point)
                        .cloned()
                    {
                        roots
                            .entry(id)
                            .or_insert_with(|| compact_packed(&out_point));
                    }
                    if let Some(ids) = tx_pool.pool_map.out_point_index.get_deps_ref(&out_point) {
                        for id in ids {
                            roots
                                .entry(id.clone())
                                .or_insert_with(|| compact_packed(&out_point));
                        }
                    }
                }

                let mut removal_ids: HashSet<_> = roots.keys().cloned().collect();
                for root in roots.keys() {
                    removal_ids.extend(tx_pool.pool_map.calc_descendants(root));
                }
                let removal_hashes: HashSet<_> = removal_ids
                    .iter()
                    .filter_map(|id| {
                        tx_pool
                            .pool_map
                            .get_by_id(id)
                            .map(|entry| entry.inner.transaction().hash())
                    })
                    .filter(|hash| !tx_pool.snapshot().transaction_exists(hash))
                    .collect();
                kernel
                    .parents_unavailable(&removal_hashes)
                    .expect("planned reorg cascade parent demotion");

                let mut ordered_roots: Vec<_> = roots.into_iter().collect();
                ordered_roots
                    .sort_by(|(left, _), (right, _)| left.as_slice().cmp(right.as_slice()));
                let mut effects = Vec::new();
                let mut available_outpoints = Vec::new();
                for (child_id, out_point) in ordered_roots {
                    let removed = tx_pool.pool_map.remove_entry_and_descendants(&child_id);
                    available_outpoints
                        .extend(tx_pool.released_inputs_from_removed_entries(&removed));
                    for entry in removed {
                        debug!(
                            "cascade-remove pool tx {}: its reference {:?} died with the failed re-add",
                            entry.transaction().hash(),
                            out_point,
                        );
                        effects.extend(self.rejected_effects(
                            entry,
                            Reject::Resolve(ckb_types::core::error::OutPointError::Dead(
                                out_point.clone(),
                            )),
                        ));
                    }
                }
                kernel
                    .note_available(crate::service::pipeline_ops::available_cell_dependencies(
                        &tx_pool,
                        available_outpoints,
                    ))
                    .expect("planned reorg cascade availability update");
                ((), EffectBatch::new(effects))
                        },
                    )
                });
                if let Err(error) = result {
                    ckb_logger::error!("reorg cascade effect journal unavailable: {error:?}");
                }
            },
        );
    }
}

pub(crate) fn update_tx_pool_for_reorg(
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

    // One-shot post-startup reconcile for entries committed (or zombied)
    // while their reorg notifications were skipped during the startup
    // reload. This runs against the fresh snapshot swapped in above, so the
    // first reorg after startup cleans up that window; afterwards it is
    // skipped — a full scan with a store lookup per entry is too expensive
    // to repeat on every block. The removed entries are returned to the
    // caller: they freed their inputs, so the same RBF-registration
    // cleanup that runs for the reject events must also run for them
    // (otherwise ghost registrations block future replacements forever).
    let mut silently_removed = Vec::new();
    if !tx_pool.onchain_reconcile_done {
        tx_pool.onchain_reconcile_done = true;
        silently_removed = tx_pool.remove_onchain_entries();
        if !silently_removed.is_empty() {
            debug!(
                "reconcile dropped {} on-chain pool entries",
                silently_removed.len()
            );
        }
    }

    // Remove transactions from the pool until its size <= size_limit.
    let current_reject = tx_pool.limit_size(None, &mut reject_events);
    debug_assert!(
        current_reject.is_none(),
        "reorg size reconciliation has no distinguished incoming entry"
    );

    // Notifications are published only after this whole pool mutation. Do
    // not export intermediate Pending -> Proposed steps, or a Pending event
    // for an entry that expiry/size reconciliation removed later in the same
    // transaction. Coalesce by full hash and read the final authoritative
    // status; a short-id collision must never attribute another transaction.
    let rejected_hashes: HashSet<_> = reject_events
        .iter()
        .map(|(entry, _)| entry.transaction().hash())
        .collect();
    let mut positions = HashMap::new();
    let mut stable_notify_events: Vec<(TxEntry, Status)> = Vec::new();
    for (entry, _) in notify_events {
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

    Ok(ReorgOutcome {
        reject_events,
        silently_removed,
        notify_events: stable_notify_events,
    })
}

#[cfg(test)]
#[path = "tests/reorg.rs"]
mod tests;
