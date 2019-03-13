use crate::block::Block;
use crate::header::Header;
use crate::transaction::ProposalShortId;
use crate::BlockNumber;
use bincode::serialize;
use hash::sha3_256;
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
}

pub fn uncles_hash(uncles: &[UncleBlock]) -> H256 {
    if uncles.is_empty() {
        H256::zero()
    } else {
        sha3_256(serialize(uncles).unwrap()).into()
    }
}
