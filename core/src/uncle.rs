use crate::block::Block;
use crate::header::Header;
use crate::transaction::ProposalShortId;
use crate::BlockNumber;
use bincode::serialize;
use ckb_merkle_tree::merkle_root;
use hash::blake2b_256;
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Default, Debug)]
pub struct UncleBlock {
    pub header: Header,
    pub proposal_transactions: Vec<ProposalShortId>,
}

impl From<Block> for UncleBlock {
    fn from(block: Block) -> Self {
        UncleBlock {
            header: block.header().clone(),
            proposal_transactions: block.proposal_transactions().to_vec(),
        }
    }
}

impl UncleBlock {
    pub fn new(header: Header, proposal_transactions: Vec<ProposalShortId>) -> UncleBlock {
        UncleBlock {
            header,
            proposal_transactions,
        }
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn number(&self) -> BlockNumber {
        self.header.number()
    }

    pub fn proposal_transactions(&self) -> &[ProposalShortId] {
        &self.proposal_transactions
    }

    pub fn cal_txs_proposal_root(&self) -> H256 {
        merkle_root(
            &self
                .proposal_transactions
                .iter()
                .map(ProposalShortId::hash)
                .collect::<Vec<_>>(),
        )
    }
}

pub fn uncles_hash(uncles: &[UncleBlock]) -> H256 {
    if uncles.is_empty() {
        H256::zero()
    } else {
        blake2b_256(serialize(uncles).expect("Uncle serialize should not fail")).into()
    }
}
