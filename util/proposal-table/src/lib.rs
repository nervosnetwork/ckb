//! The ckb proposal-table design for two-step-transaction-confirmation

use ckb_chain_spec::consensus::ProposalWindow;
use ckb_types::{core::BlockNumber, packed::ProposalShortId};
use std::collections::{BTreeMap, HashSet};
use std::ops::Bound;

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
                    .map(|pair| pair.1)
                    .cloned()
                    .flatten()
                    .collect(),
            )
        } else {
            (
                self.table
                    .range((
                        Bound::Included(&proposal_start),
                        Bound::Included(&proposal_end),
                    ))
                    .map(|pair| pair.1)
                    .cloned()
                    .flatten()
                    .collect(),
                self.table
                    .range((Bound::Excluded(&proposal_end), Bound::Included(&number)))
                    .map(|pair| pair.1)
                    .cloned()
                    .flatten()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::iter;

    #[test]
    fn test_finalize() {
        let proposals = vec![
            ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
            ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 2]),
            ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 3]),
            ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 4]),
            ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 5]),
            ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 6]),
            ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 7]),
            ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 8]),
            ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 9]),
            ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 10]),
        ];

        let window = ProposalWindow(2, 10);
        let mut table = ProposalTable::new(window);

        for (idx, id) in proposals.iter().skip(1).enumerate() {
            let mut ids = HashSet::new();
            ids.insert(id.clone());
            table.insert((idx + 1) as u64, ids.clone());
        }

        let (removed_ids, mut view) = table.finalize(&ProposalView::default(), 1);
        assert!(removed_ids.is_empty());
        assert!(view.set().is_empty());
        assert_eq!(view.gap(), &iter::once(proposals[1].clone()).collect());

        // in window
        for i in 2..=10usize {
            let (removed_ids, new_view) = table.finalize(&view, i as u64);
            let c = i + 1;
            assert_eq!(
                new_view.gap(),
                &proposals[(c - 2 + 1)..=i].iter().cloned().collect()
            );

            let s = ::std::cmp::max(1, c.saturating_sub(10));
            assert_eq!(
                new_view.set(),
                &proposals[s..=(c - 2)].iter().cloned().collect()
            );

            assert!(removed_ids.is_empty());
            view = new_view;
        }

        // finalize 11
        let (removed_ids, new_view) = table.finalize(&view, 11);
        assert_eq!(removed_ids, iter::once(proposals[1].clone()).collect());
        assert_eq!(new_view.set(), &proposals[2..=10].iter().cloned().collect());
        assert!(new_view.gap().is_empty());

        view = new_view;

        // finalize 12
        let (removed_ids, new_view) = table.finalize(&view, 12);
        assert_eq!(removed_ids, iter::once(proposals[2].clone()).collect());
        assert_eq!(new_view.set(), &proposals[3..=10].iter().cloned().collect());
        assert!(new_view.gap().is_empty());
    }
}
