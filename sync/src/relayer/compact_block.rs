use bigint::H48;
use ckb_protocol::{self, FlatbuffersVectorIterator};
use core::header::Header;
use core::transaction::{ProposalShortId, Transaction};
use core::uncle::UncleBlock;

pub type ShortTransactionID = H48;

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CompactBlock {
    pub header: Header,
    pub uncles: Vec<UncleBlock>,
    pub nonce: u64,
    pub short_ids: Vec<ShortTransactionID>,
    pub prefilled_transactions: Vec<PrefilledTransaction>,
    pub proposal_transactions: Vec<ProposalShortId>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct PrefilledTransaction {
    pub index: usize,
    pub transaction: Transaction,
}

impl<'a> From<ckb_protocol::CompactBlock<'a>> for CompactBlock {
    fn from(b: ckb_protocol::CompactBlock<'a>) -> Self {
        CompactBlock {
            header: b.header().unwrap().into(),
            nonce: b.nonce(),
            short_ids: FlatbuffersVectorIterator::new(b.short_ids().unwrap())
                .map(|bytes| ShortTransactionID::from(bytes.seq().unwrap()))
                .collect(),
            prefilled_transactions: FlatbuffersVectorIterator::new(
                b.prefilled_transactions().unwrap(),
            ).map(Into::into)
            .collect(),

            uncles: FlatbuffersVectorIterator::new(b.uncles().unwrap())
                .map(Into::into)
                .collect(),

            proposal_transactions: FlatbuffersVectorIterator::new(
                b.proposal_transactions().unwrap(),
            ).filter_map(|bytes| ProposalShortId::from_slice(bytes.seq().unwrap()))
            .collect(),
        }
    }
}

impl<'a> From<ckb_protocol::PrefilledTransaction<'a>> for PrefilledTransaction {
    fn from(pt: ckb_protocol::PrefilledTransaction<'a>) -> Self {
        PrefilledTransaction {
            index: pt.index() as usize,
            transaction: pt.transaction().unwrap().into(),
        }
    }
}
