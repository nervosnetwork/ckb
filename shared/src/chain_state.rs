use crate::tx_proposal_table::TxProposalTable;
use crate::txo_set::{TxoSet, TxoSetDiff};
use ckb_core::block::Block;
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{OutPoint, ProposalShortId};
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;

#[derive(Default, Debug, PartialEq, Clone, Eq)]
pub struct ChainState {
    tip_header: Header,
    total_difficulty: U256,
    txo_set: TxoSet,
    proposal_ids: TxProposalTable,
}

impl ChainState {
    pub fn new(
        tip_header: Header,
        total_difficulty: U256,
        txo_set: TxoSet,
        proposal_ids: TxProposalTable,
    ) -> Self {
        ChainState {
            tip_header,
            total_difficulty,
            txo_set,
            proposal_ids,
        }
    }

    pub fn tip_number(&self) -> BlockNumber {
        self.tip_header.number()
    }

    pub fn tip_hash(&self) -> H256 {
        self.tip_header.hash()
    }

    pub fn total_difficulty(&self) -> &U256 {
        &self.total_difficulty
    }

    pub fn tip_header(&self) -> &Header {
        &self.tip_header
    }

    pub fn txo_set(&self) -> &TxoSet {
        &self.txo_set
    }

    pub fn is_spent(&self, o: &OutPoint) -> Option<bool> {
        self.txo_set.is_spent(o)
    }

    pub fn contains_proposal_id(&self, id: &ProposalShortId) -> bool {
        self.proposal_ids.contains(id)
    }

    pub fn update_proposal_ids(&mut self, block: &Block) {
        self.proposal_ids
            .update_or_insert(block.header().number(), block.union_proposal_ids())
    }

    pub fn get_proposal_ids_iter(&self) -> impl Iterator<Item = &ProposalShortId> {
        self.proposal_ids.get_ids_iter()
    }

    pub fn reconstruct_proposal_ids(&mut self, number: BlockNumber) -> Vec<ProposalShortId> {
        self.proposal_ids.reconstruct(number)
    }

    pub fn update_tip(&mut self, header: Header, total_difficulty: U256, txo_diff: TxoSetDiff) {
        self.tip_header = header;
        self.total_difficulty = total_difficulty;
        self.txo_set.update(txo_diff);
    }
}
