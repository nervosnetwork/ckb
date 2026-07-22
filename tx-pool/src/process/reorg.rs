use crate::component::entry::TxEntry;
use crate::component::pool_map::Status;
use crate::error::Reject;
use crate::pool::TxPool;
use ckb_logger::debug;
use ckb_snapshot::Snapshot;
use ckb_types::core::TransactionView;
use ckb_types::packed::{Byte32, ProposalShortId};
use ckb_util::LinkedHashSet;
use std::collections::HashSet;
use std::sync::Arc;

/// Collected results of [`update_tx_pool_for_reorg`], all dispatched by
/// the caller outside the write lock.
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

pub(crate) fn update_tx_pool_for_reorg(
    tx_pool: &mut TxPool,
    attached: &LinkedHashSet<TransactionView>,
    detached_headers: &HashSet<Byte32>,
    detached_proposal_id: HashSet<ProposalShortId>,
    snapshot: Arc<Snapshot>,
    mine_mode: bool,
) -> ReorgOutcome {
    let mut reject_events = Vec::new();
    // Proposed/pending notifications are *collected* and dispatched by the
    // caller outside the write lock: running user callbacks while holding
    // the tx-pool write lock would let a blocking callback stall (or
    // re-enter and deadlock) the whole pool.
    let mut notify_events = Vec::new();

    tx_pool.snapshot = Arc::clone(&snapshot);

    // NOTE: `remove_by_detached_proposal` will try to re-put the given expired/detached proposals into
    // pending-pool if they can be found within txpool. As for a transaction
    // which is both expired and committed at the one time(commit at its end of commit-window),
    // we should treat it as a committed and not re-put into pending-pool. So we should ensure
    // that involves `remove_committed_txs` before `remove_expired`.
    tx_pool.remove_committed_txs(attached.iter(), detached_headers, &mut reject_events);
    tx_pool.remove_by_detached_proposal(
        detached_proposal_id.iter(),
        &mut notify_events,
        &mut reject_events,
    );

    // mine mode:
    // pending ---> gap ----> proposed
    // try move gap to proposed
    if mine_mode {
        let mut proposals = Vec::new();
        let mut gaps = Vec::new();

        for entry in tx_pool.pool_map.entries.get_by_status(&Status::Gap) {
            let short_id = entry.inner.proposal_short_id();
            if snapshot.proposals().contains_proposed(&short_id) {
                proposals.push((short_id, entry.inner.clone()));
            }
        }

        for entry in tx_pool.pool_map.entries.get_by_status(&Status::Pending) {
            let short_id = entry.inner.proposal_short_id();
            let elem = (short_id.clone(), entry.inner.clone());
            if snapshot.proposals().contains_proposed(&short_id) {
                proposals.push(elem);
            } else if snapshot.proposals().contains_gap(&short_id) {
                gaps.push(elem);
            }
        }

        for (id, entry) in proposals {
            debug!("begin to proposed: {:x}", id);
            if let Err(e) = tx_pool.proposed_rtx(&id) {
                // The entry was NOT removed — it stays in the pool — so a
                // transition failure must not surface as a rejection event:
                // subscribers would see a tx rejected while it is still
                // pending. Currently unreachable (the entries were read
                // from the pool under the same write lock); log only.
                ckb_logger::error!(
                    "Failed to add proposed tx {}, reason: {}",
                    entry.transaction().hash(),
                    e
                );
            } else {
                notify_events.push((entry, Status::Proposed));
            }
        }

        for (id, entry) in gaps {
            debug!("begin to gap: {:x}", id);
            if let Err(e) = tx_pool.gap_rtx(&id) {
                ckb_logger::error!(
                    "Failed to add tx to gap {}, reason: {}",
                    entry.transaction().hash(),
                    e
                );
            }
        }
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
    let _ = tx_pool.limit_size(None, &mut reject_events);

    ReorgOutcome {
        reject_events,
        silently_removed,
        notify_events,
    }
}
