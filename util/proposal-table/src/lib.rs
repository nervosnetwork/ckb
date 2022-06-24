//! The ckb proposal-table design for two-step-transaction-confirmation

use ckb_chain_spec::consensus::ProposalWindow;
use ckb_types::{core::BlockNumber, packed::ProposalShortId};
use std::collections::{BTreeMap, HashSet};
use std::ops::Bound;

#[cfg(test)]
mod tests;

/// A view captures point-time proposal set, representing on-chain proposed transaction pool,
/// stored in the memory so that there is no need to fetch on hard disk, create by ProposalTable finalize method
/// w_close and w_far define the closest and farthest on-chain distance between a transaction’s proposal and commitment.
#[derive(Default, Clone, Debug)]
pub struct ProposalView {
    pub(crate) gap: HashSet<ProposalShortId>,
    pub(crate) set: HashSet<ProposalShortId>,
}

impl ProposalView {
    /// Create new ProposalView
    pub fn new(gap: HashSet<ProposalShortId>, set: HashSet<ProposalShortId>) -> ProposalView {
        ProposalView { gap, set }
    }

    /// Return proposals between w_close and tip
    pub fn gap(&self) -> &HashSet<ProposalShortId> {
        &self.gap
    }

    /// Return proposals between w_close and w_far
    pub fn set(&self) -> &HashSet<ProposalShortId> {
        &self.set
    }

    /// Returns true if the proposals set between w_close and w_far contains the id.
    pub fn contains_proposed(&self, id: &ProposalShortId) -> bool {
        self.set.contains(id)
    }

    /// Returns true if the proposals set between w_close and tip contains the id.
    pub fn contains_gap(&self, id: &ProposalShortId) -> bool {
        self.gap.contains(id)
    }
}

/// A Table record proposals set in number-ids pairs
#[derive(Debug, PartialEq, Clone, Eq)]
pub struct ProposalTable {
    pub(crate) table: BTreeMap<BlockNumber, HashSet<ProposalShortId>>,
    pub(crate) proposal_window: ProposalWindow,
}

impl ProposalTable {
    /// Create new ProposalTable from ProposalWindow
    pub fn new(proposal_window: ProposalWindow) -> Self {
        ProposalTable {
            proposal_window,
            table: BTreeMap::default(),
        }
    }

    /// Inserts a number-ids pair into the table.
    /// If the TABLE did not have this number present, true is returned.
    /// If the map did have this number present, the proposal set is updated.
    pub fn insert(&mut self, number: BlockNumber, ids: HashSet<ProposalShortId>) -> bool {
        self.table.insert(number, ids).is_none()
    }

    /// Removes a proposal set from the table,　returning the set at the number if the number was previously in the table
    ///
    /// # Examples
    ///
    /// ```
    /// use ckb_chain_spec::consensus::ProposalWindow;
    /// use ckb_proposal_table::ProposalTable;
    ///
    /// let window = ProposalWindow(2, 10);
    /// let mut table = ProposalTable::new(window);
    /// assert_eq!(table.remove(1), None);
    /// ```
    pub fn remove(&mut self, number: BlockNumber) -> Option<HashSet<ProposalShortId>> {
        self.table.remove(&number)
    }

    /// Return referent of internal BTreeMap contains all proposal set
    pub fn all(&self) -> &BTreeMap<BlockNumber, HashSet<ProposalShortId>> {
        &self.table
    }

    /// Update table by proposal window move froward, drop outdated proposal set
    /// Return removed proposal ids set and new ProposalView
    pub fn finalize(
        &mut self,
        origin: &ProposalView,
        number: BlockNumber,
    ) -> (HashSet<ProposalShortId>, ProposalView) {
        let candidate_number = number + 1;
        let proposal_start = candidate_number.saturating_sub(self.proposal_window.farthest());
        let proposal_end = candidate_number.saturating_sub(self.proposal_window.closest());

        if proposal_start > 1 {
            self.table = self.table.split_off(&proposal_start);
        }

        ckb_logger::trace!("[proposal_finalize] table {:?}", self.table);

        // - if candidate_number <= self.proposal_window.closest()
        //      new_ids = []
        //      gap = [1..candidate_number]
        // - else
        //      new_ids = [candidate_number- farthest..= candidate_number- closest]
        //      gap = [candidate_number- closest + 1..candidate_number]
        // - end
        let (new_ids, gap) = if candidate_number <= self.proposal_window.closest() {
            (
                HashSet::new(),
                self.table
                    .range((Bound::Unbounded, Bound::Included(&number)))
                    .flat_map(|pair| pair.1)
                    .cloned()
                    .collect(),
            )
        } else {
            (
                self.table
                    .range((
                        Bound::Included(&proposal_start),
                        Bound::Included(&proposal_end),
                    ))
                    .flat_map(|pair| pair.1)
                    .cloned()
                    .collect(),
                self.table
                    .range((Bound::Excluded(&proposal_end), Bound::Included(&number)))
                    .flat_map(|pair| pair.1)
                    .cloned()
                    .collect(),
            )
        };

        let removed_ids: HashSet<ProposalShortId> =
            origin.set().difference(&new_ids).cloned().collect();
        ckb_logger::trace!(
            "[proposal_finalize] number {} proposal_start {}----proposal_end {}",
            number,
            proposal_start,
            proposal_end
        );
        ckb_logger::trace!(
            "[proposal_finalize] number {} new_ids {:?}----removed_ids {:?}",
            number,
            new_ids,
            removed_ids
        );
        (removed_ids, ProposalView::new(gap, new_ids))
    }
}
