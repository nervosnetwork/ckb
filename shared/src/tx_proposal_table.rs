use ckb_chain_spec::consensus::ProposalWindow;
use ckb_logger::trace_target;
use ckb_types::{core::BlockNumber, packed::ProposalShortId};
use std::collections::{BTreeMap, HashSet};
use std::ops::Bound;

#[derive(Debug, PartialEq, Clone, Eq)]
pub struct TxProposalTable {
    pub(crate) table: BTreeMap<BlockNumber, HashSet<ProposalShortId>>,
    pub(crate) gap: HashSet<ProposalShortId>,
    pub(crate) set: HashSet<ProposalShortId>,
    pub(crate) proposal_window: ProposalWindow,
}

impl TxProposalTable {
    pub fn new(proposal_window: ProposalWindow) -> Self {
        TxProposalTable {
            proposal_window,
            gap: HashSet::default(),
            set: HashSet::default(),
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

    pub fn contains(&self, id: &ProposalShortId) -> bool {
        self.set.contains(id)
    }

    pub fn contains_gap(&self, id: &ProposalShortId) -> bool {
        self.gap.contains(id)
    }

    pub fn get_ids_iter(&self) -> impl Iterator<Item = &ProposalShortId> {
        self.set.iter()
    }

    pub fn all(&self) -> &BTreeMap<BlockNumber, HashSet<ProposalShortId>> {
        &self.table
    }

    pub fn finalize(&mut self, number: BlockNumber) -> HashSet<ProposalShortId> {
        let proposal_start = number.saturating_sub(self.proposal_window.farthest()) + 1;
        let proposal_end = number.saturating_sub(self.proposal_window.closest()) + 1;
        self.table = self.table.split_off(&proposal_start);

        trace_target!(
            crate::LOG_TARGET_CHAIN,
            "[proposal_finalize] table {:?}",
            self.table
        );
        let new_ids = self
            .table
            .range((Bound::Unbounded, Bound::Included(&proposal_end)))
            .map(|pair| pair.1)
            .cloned()
            .flatten()
            .collect();

        self.gap = self
            .table
            .range((Bound::Excluded(&proposal_end), Bound::Unbounded))
            .map(|pair| pair.1)
            .cloned()
            .flatten()
            .collect();

        let removed_ids: HashSet<ProposalShortId> =
            self.set.difference(&new_ids).cloned().collect();
        trace_target!(
            crate::LOG_TARGET_CHAIN,
            "[proposal_finalize] number {} proposal_start {}----proposal_end {}",
            number,
            proposal_start,
            proposal_end
        );
        trace_target!(
            crate::LOG_TARGET_CHAIN,
            "[proposal_finalize] number {} new_ids {:?}----removed_ids {:?}",
            number,
            new_ids,
            removed_ids
        );
        self.set = new_ids;
        removed_ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_finalize() {
        let id = ProposalShortId::zero();
        let window = ProposalWindow(2, 10);
        let mut table = TxProposalTable::new(window);
        let mut ids = HashSet::default();
        ids.insert(id.clone());
        table.insert(1, ids.clone());
        assert!(!table.contains(&id));

        // in window
        for i in 2..10 {
            assert!(table.finalize(i).is_empty());
            assert!(table.contains(&id));
        }

        assert_eq!(table.finalize(11), ids);
        assert!(!table.contains(&id));

        assert!(table.finalize(12).is_empty());
        assert!(!table.contains(&id));
    }
}
