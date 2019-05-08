use ckb_chain_spec::consensus::ProposalWindow;
use ckb_core::header::BlockNumber;
use ckb_core::script::Script;
use ckb_core::transaction::ProposalShortId;
use ckb_util::FnvHashSet;
use log::trace;
use std::collections::BTreeMap;
use std::ops::Bound;

#[derive(Debug, PartialEq, Clone, Eq)]
pub struct TxProposalTable {
    pub(crate) tip: BlockNumber,
    pub(crate) table: BTreeMap<BlockNumber, FnvHashSet<ProposalShortId>>,
    pub(crate) proposers: BTreeMap<BlockNumber, Script>,
    pub(crate) set: FnvHashSet<ProposalShortId>,
    pub(crate) proposal_window: ProposalWindow,
}

impl TxProposalTable {
    pub fn new(tip: BlockNumber, proposal_window: ProposalWindow) -> Self {
        TxProposalTable {
            tip,
            proposal_window,
            set: FnvHashSet::default(),
            table: BTreeMap::default(),
            proposers: BTreeMap::default(),
        }
    }

    // If the TABLE did not have this value present, true is returned.
    // If the TABLE did have this value present, false is returned
    pub fn insert(
        &mut self,
        number: BlockNumber,
        ids: FnvHashSet<ProposalShortId>,
        proposer: Script,
    ) {
        self.table.insert(number, ids);
        self.proposers.insert(number, proposer);
    }

    pub fn remove(&mut self, number: BlockNumber) -> Option<FnvHashSet<ProposalShortId>> {
        self.table.remove(&number)
    }

    pub fn contains(&self, id: &ProposalShortId) -> bool {
        self.set.contains(id)
    }

    pub fn get_proposer_by_id(&self, id: &ProposalShortId) -> Option<Script> {
        let proposal_end = self.tip.saturating_sub(self.proposal_window.end()) + 1;
        let item = self
            .table
            .range((Bound::Unbounded, Bound::Included(&proposal_end)))
            .find(|(_, ids)| ids.contains(id));
        item.and_then(|(n, _)| self.proposers.get(n)).cloned()
    }

    pub fn get_ids_iter(
        &self,
    ) -> impl Iterator<Item = (&BlockNumber, &FnvHashSet<ProposalShortId>)> {
        let proposal_end = self.tip.saturating_sub(self.proposal_window.end()) + 1;
        self.table
            .range((Bound::Unbounded, Bound::Included(&proposal_end)))
    }

    pub fn all(&self) -> &BTreeMap<BlockNumber, FnvHashSet<ProposalShortId>> {
        &self.table
    }

    pub fn finalize(&mut self, number: BlockNumber) -> FnvHashSet<ProposalShortId> {
        self.tip = number;
        let proposal_start = number.saturating_sub(self.proposal_window.start()) + 1;
        let proposal_end = number.saturating_sub(self.proposal_window.end()) + 1;

        let mut left = self.table.split_off(&proposal_start);
        ::std::mem::swap(&mut self.table, &mut left);

        let mut left = self.proposers.split_off(&proposal_start);
        ::std::mem::swap(&mut self.proposers, &mut left);

        trace!(target: "chain", "[proposal_finalize] table {:?}", self.table);
        let new_ids = self
            .table
            .range((Bound::Unbounded, Bound::Included(&proposal_end)))
            .map(|pair| pair.1)
            .cloned()
            .flatten()
            .collect();

        let removed_ids: FnvHashSet<ProposalShortId> =
            self.set.difference(&new_ids).cloned().collect();
        trace!(target: "chain", "[proposal_finalize] number {} proposal_start {}----proposal_end {}", number , proposal_start, proposal_end);
        trace!(target: "chain", "[proposal_finalize] number {} new_ids {:?}----removed_ids {:?}", number, new_ids, removed_ids);
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
        let mut table = TxProposalTable::new(0, window);
        let mut ids = FnvHashSet::default();
        ids.insert(id);
        table.insert(1, ids.clone(), Script::default());
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
