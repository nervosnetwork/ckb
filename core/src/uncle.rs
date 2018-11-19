use super::header::Header;
use super::transaction::{ProposalShortId, Transaction};
use bigint::H256;
use bincode::serialize;
use ckb_protocol;
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

impl<'a> From<&'a ckb_protocol::UncleBlock> for UncleBlock {
    fn from(proto: &'a ckb_protocol::UncleBlock) -> Self {
        UncleBlock {
            header: proto.get_header().into(),
            cellbase: proto.get_cellbase().into(),
            proposal_transactions: proto
                .get_proposal_transactions()
                .iter()
                .filter_map(|id| ProposalShortId::from_slice(&id))
                .collect(),
        }
    }
}

impl<'a> From<&'a UncleBlock> for ckb_protocol::UncleBlock {
    fn from(uncle: &'a UncleBlock) -> Self {
        let mut proto = ckb_protocol::UncleBlock::new();
        proto.set_header(uncle.header().into());
        proto.set_cellbase(uncle.cellbase().into());
        let proposal_transactions = uncle
            .proposal_transactions()
            .iter()
            .map(|t| t.to_vec())
            .collect();
        proto.set_proposal_transactions(proposal_transactions);
        proto
    }
}
