use ckb_chain_spec::consensus::ProposalWindow;
use ckb_types::{core::BlockNumber, packed::ProposalShortId};
use std::collections::{BTreeMap, HashSet};
use std::ops::Bound;

#[derive(Default, Clone, Debug)]
pub struct ProposalView {
    pub(crate) gap: HashSet<ProposalShortId>,
    pub(crate) set: HashSet<ProposalShortId>,
}

impl ProposalView {
    pub fn new(gap: HashSet<ProposalShortId>, set: HashSet<ProposalShortId>) -> ProposalView {
        ProposalView { gap, set }
    }

    pub fn gap(&self) -> &HashSet<ProposalShortId> {
        &self.gap
    }

    pub fn set(&self) -> &HashSet<ProposalShortId> {
        &self.set
    }

    pub fn contains_proposed(&self, id: &ProposalShortId) -> bool {
        self.set.contains(id)
    }

    pub fn contains_gap(&self, id: &ProposalShortId) -> bool {
        self.gap.contains(id)
    }
}

#[derive(Debug, PartialEq, Clone, Eq)]
pub struct ProposalTable {
    pub(crate) table: BTreeMap<BlockNumber, HashSet<ProposalShortId>>,
    pub(crate) proposal_window: ProposalWindow,
}

impl ProposalTable {
    pub fn new(proposal_window: ProposalWindow) -> Self {
        ProposalTable {
            proposal_window,
            table: BTreeMap::default(),
        }
    }

    // If the TABLE did not have this value present, true is returned.
    // If the TABLE did have this value present, false is returned
    pub fn insert(&mut self, number: BlockNumber, ids: HashSet<ProposalShortId>) -> bool {
        self.table.insert(number, ids).is_none()
    }

    pub fn remove(&mut self, number: BlockNumber) -> Option<HashSet<ProposalShortId>> {
        self.table.remove(&number)
    }

    pub fn all(&self) -> &BTreeMap<BlockNumber, HashSet<ProposalShortId>> {
        &self.table
    }

    pub fn finalize(
        &mut self,
        origin: &ProposalView,
        number: BlockNumber,
    ) -> (HashSet<ProposalShortId>, ProposalView) {
        let proposal_start = number.saturating_sub(self.proposal_window.farthest()) + 1;
        let proposal_end = number.saturating_sub(self.proposal_window.closest()) + 1;

        self.table = self.table.split_off(&proposal_start);

        ckb_logger::trace!("[proposal_finalize] table {:?}", self.table);
        let new_ids = self
            .table
            .range((Bound::Unbounded, Bound::Included(&proposal_end)))
            .map(|pair| pair.1)
            .cloned()
            .flatten()
            .collect();

        let gap = self
            .table
            .range((Bound::Excluded(&proposal_end), Bound::Unbounded))
            .map(|pair| pair.1)
            .cloned()
            .flatten()
            .collect();

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

    #[test]
    fn test_finalize() {
        let id = ProposalShortId::zero();
        let window = ProposalWindow(2, 10);
        let mut table = ProposalTable::new(window);
        let mut ids = HashSet::new();
        ids.insert(id.clone());
        table.insert(1, ids.clone());
        let (_, mut view) = table.finalize(&ProposalView::default(), 1);

        // in window
        for i in 2..10 {
            let (removed_ids, new_view) = table.finalize(&view, i);
            assert!(removed_ids.is_empty());
            assert!(new_view.contains_proposed(&id));
            view = new_view;
        }

        let (removed_ids, new_view) = table.finalize(&view, 11);
        assert_eq!(removed_ids, ids);
        assert!(!new_view.contains_proposed(&id));
        view = new_view;

        let (removed_ids, new_view) = table.finalize(&view, 12);
        assert!(removed_ids.is_empty());
        assert!(!new_view.contains_proposed(&id));
    }
}
