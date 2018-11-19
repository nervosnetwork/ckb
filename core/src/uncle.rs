use super::header::Header;
use super::transaction::{ProposalShortId, Transaction};
use bigint::H256;
use bincode::serialize;
use hash::sha3_256;
use BlockNumber;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Default, Debug)]
pub struct UncleBlock {
    pub header: Header,
    pub cellbase: Transaction,
    pub proposal_transactions: Vec<ProposalShortId>,
}

impl UncleBlock {
    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn cellbase(&self) -> &Transaction {
        &self.cellbase
    }

    pub fn number(&self) -> BlockNumber {
        self.header.number
    }

    pub fn proposal_transactions(&self) -> &[ProposalShortId] {
        &self.proposal_transactions
    }
}

pub fn uncles_hash(uncles: &[UncleBlock]) -> H256 {
    sha3_256(serialize(uncles).unwrap()).into()
}
