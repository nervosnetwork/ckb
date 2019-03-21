use ckb_core::header::Header;
use ckb_core::transaction::{IndexTransaction, ProposalShortId};
use ckb_core::uncle::UncleBlock;
use ckb_protocol::{self, cast, FlatbuffersVectorIterator};
use ckb_util::{TryFrom, TryInto};
use failure::Error as FailureError;

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

impl<'a> TryFrom<ckb_protocol::CompactBlock<'a>> for CompactBlock {
    type Error = FailureError;

    fn try_from(b: ckb_protocol::CompactBlock<'a>) -> Result<Self, Self::Error> {
        let header = cast!(b.header())?;
        let short_ids = cast!(b.short_ids())?;
        let prefilled_transactions: Result<Vec<_>, FailureError> =
            FlatbuffersVectorIterator::new(cast!(b.prefilled_transactions())?)
                .map(TryInto::try_into)
                .collect();

        let uncles: Result<Vec<_>, FailureError> =
            FlatbuffersVectorIterator::new(cast!(b.uncles())?)
                .map(TryInto::try_into)
                .collect();
        let proposal_transactions: Result<Vec<_>, FailureError> = cast!(b.proposal_transactions())?
            .iter()
            .map(TryInto::try_into)
            .collect();

        Ok(CompactBlock {
            header: header.try_into()?,
            nonce: b.nonce(),
            short_ids: cast!(FlatbuffersVectorIterator::new(short_ids)
                .map(|bytes| bytes.seq().map(|seq| {
                    let mut short_id = [0u8; 6];
                    short_id.copy_from_slice(seq);
                    short_id
                }))
                .collect::<Option<Vec<_>>>())?,
            prefilled_transactions: prefilled_transactions?,
            uncles: uncles?,
            proposal_transactions: proposal_transactions?,
        })
    }
}
