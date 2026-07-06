use crate::component::entry::TxEntry;
use crate::error::Reject;
use crate::pool::TxPool;
use ckb_logger::debug;
use ckb_types::packed::ProposalShortId;
use std::collections::HashSet;

impl super::TxPoolService {
    // Remove conflicting transactions for RBF and record them in the conflicts
    // cache so they can be recovered if the replacement fails. Returns the set
    // of removed entries; the caller decides which ones to recover and when to
    // clean up the conflicts cache.
    pub(crate) fn process_rbf(
        &self,
        tx_pool: &mut TxPool,
        entry: &TxEntry,
        conflicts: &HashSet<ProposalShortId>,
        reject_events: &mut Vec<(TxEntry, Reject)>,
    ) -> Vec<TxEntry> {
        if conflicts.is_empty() {
            return Vec::new();
        }

        let all_removed: Vec<_> = conflicts
            .iter()
            .flat_map(|id| tx_pool.pool_map.remove_entry_and_descendants(id))
            .collect();

        for old in &all_removed {
            debug!(
                "remove conflict tx {} for RBF by new tx {}",
                old.transaction().hash(),
                entry.transaction().hash()
            );
            let reject =
                Reject::RBFRejected(format!("replaced by tx {}", entry.transaction().hash()));

            // collect reject events for dispatch outside write lock
            reject_events.push((old.clone(), reject));
        }

        // Record every removed entry (direct conflicts and their descendants)
        // in the conflicts cache so that they can all be recovered if the
        // replacement fails or if their inputs become available again.
        for old in &all_removed {
            tx_pool.record_conflict(old.transaction().clone());
        }

        all_removed
    }
}
