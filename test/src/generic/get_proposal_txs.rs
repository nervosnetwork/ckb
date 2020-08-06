use ckb_types::core::{BlockView, TransactionView};
use ckb_types::packed::{Block, ProposalShortId, Transaction};

pub trait GetProposalTxId {
    fn get_proposal_tx_id(&self) -> ProposalShortId;
}

impl GetProposalTxId for TransactionView {
    fn get_proposal_tx_id(&self) -> ProposalShortId {
        self.proposal_short_id()
    }
}

impl GetProposalTxId for Transaction {
    fn get_proposal_tx_id(&self) -> ProposalShortId {
        self.proposal_short_id()
    }
}

impl GetProposalTxId for ProposalShortId {
    fn get_proposal_tx_id(&self) -> ProposalShortId {
        self.clone()
    }
}

pub trait GetProposalTxIds {
    fn get_proposal_tx_ids(&self) -> Vec<ProposalShortId>;
}

impl<T> GetProposalTxIds for T
where
    T: GetProposalTxId,
{
    fn get_proposal_tx_ids(&self) -> Vec<ProposalShortId> {
        vec![self.get_proposal_tx_id()]
    }
}

impl<T> GetProposalTxIds for Vec<T>
where
    T: GetProposalTxId,
{
    fn get_proposal_tx_ids(&self) -> Vec<ProposalShortId> {
        self.iter().map(|t| t.get_proposal_tx_id()).collect()
    }
}

impl GetProposalTxIds for Block {
    fn get_proposal_tx_ids(&self) -> Vec<ProposalShortId> {
        self.proposals().into_iter().collect()
    }
}

impl GetProposalTxIds for BlockView {
    fn get_proposal_tx_ids(&self) -> Vec<ProposalShortId> {
        self.data().get_proposal_tx_ids()
    }
}
