use ckb_core::header::Header;
use ckb_core::transaction::{IndexTransaction, ProposalShortId};
use ckb_core::uncle::UncleBlock;
use ckb_protocol::{self, FlatbuffersVectorIterator};

pub type ShortTransactionID = [u8; 6];

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CompactBlock {
    pub header: Header,
    pub uncles: Vec<UncleBlock>,
    pub nonce: u64,
    pub short_ids: Vec<ShortTransactionID>,
    pub prefilled_transactions: Vec<IndexTransaction>,
    pub proposal_transactions: Vec<ProposalShortId>,
}

impl<'a> From<ckb_protocol::CompactBlock<'a>> for CompactBlock {
    fn from(b: ckb_protocol::CompactBlock<'a>) -> Self {
        CompactBlock {
            header: b.header().unwrap().into(),
            nonce: b.nonce(),
            short_ids: FlatbuffersVectorIterator::new(b.short_ids().unwrap())
                .map(|bytes| {
                    let mut short_id = [0u8; 6];
                    short_id.copy_from_slice(bytes.seq().unwrap());
                    short_id
                })
                .collect(),
            prefilled_transactions: FlatbuffersVectorIterator::new(
                b.prefilled_transactions().unwrap(),
            )
            .map(Into::into)
            .collect(),

            uncles: FlatbuffersVectorIterator::new(b.uncles().unwrap())
                .map(Into::into)
                .collect(),

            proposal_transactions: FlatbuffersVectorIterator::new(
                b.proposal_transactions().unwrap(),
            )
            .filter_map(|bytes| ProposalShortId::from_slice(bytes.seq().unwrap()))
            .collect(),
        }
    }
}
