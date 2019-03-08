use ckb_chain_spec::consensus::ProposalWindow;
use ckb_core::header::BlockNumber;
use ckb_core::transaction::ProposalShortId;
use fnv::FnvHashSet;
use log::trace;
use std::collections::BTreeMap;
use std::ops::Bound;

#[derive(Default, Debug, PartialEq, Clone, Eq)]
pub struct TxProposalTable {
    pub(crate) table: BTreeMap<BlockNumber, FnvHashSet<ProposalShortId>>,
    pub(crate) set: FnvHashSet<ProposalShortId>,
    pub(crate) proposal_window: ProposalWindow,
}

impl TxProposalTable {
    pub fn new(proposal_window: ProposalWindow) -> Self {
        TxProposalTable {
            proposal_window,
            set: FnvHashSet::default(),
            table: BTreeMap::default(),
        }
    }

    pub fn update_or_insert(
        &mut self,
        number: BlockNumber,
        ids: impl IntoIterator<Item = ProposalShortId>,
    ) {
        self.table
            .entry(number)
            .or_insert_with(Default::default)
            .extend(ids);
    }

    pub fn get_ids_by_number(&self, number: BlockNumber) -> Option<&FnvHashSet<ProposalShortId>> {
        self.table.get(&number)
    }

    pub fn contains(&self, id: &ProposalShortId) -> bool {
        self.set.contains(id)
    }

    pub fn get_ids_iter(&self) -> impl Iterator<Item = &ProposalShortId> {
        self.set.iter()
    }

    pub fn reconstruct(&mut self, number: BlockNumber) -> Vec<ProposalShortId> {
        let proposal_start = number.saturating_sub(self.proposal_window.start());
        let proposal_end = number.saturating_sub(self.proposal_window.end());

        let mut left = self.table.split_off(&proposal_start);
        ::std::mem::swap(&mut self.table, &mut left);
        let new_ids = self
            .table
            .range((Bound::Unbounded, Bound::Included(&proposal_end)))
            .map(|pair| pair.1)
            .cloned()
            .flatten()
            .collect();

        let removed_ids: Vec<ProposalShortId> = self.set.difference(&new_ids).cloned().collect();
        trace!(target: "chain", "[proposal_reconstruct] proposal_start {}----proposal_end {}", proposal_start, proposal_end);
        trace!(target: "chain", "[proposal_reconstruct] new_ids {:?}----removed_ids {:?}", new_ids, removed_ids);
        self.set = new_ids;
        removed_ids
    }
}
