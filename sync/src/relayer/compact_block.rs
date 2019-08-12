use ckb_core::header::{Header, HeaderBuilder};
use ckb_core::transaction::{IndexTransaction, ProposalShortId, Transaction};
use ckb_core::uncle::UncleBlock;
use ckb_core::Cycle;
use ckb_protocol::{self, cast, FlatbuffersVectorIterator};
use failure::Error as FailureError;
use numext_fixed_hash::H256;
use std::collections::HashSet;
use std::convert::{TryFrom, TryInto};

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CompactBlock {
    pub header: Header,
    pub uncles: Vec<UncleBlock>,
    pub short_ids: Vec<ProposalShortId>,
    pub prefilled_transactions: Vec<IndexTransaction>,
    pub proposals: Vec<ProposalShortId>,
}

impl Default for CompactBlock {
    fn default() -> Self {
        let header = HeaderBuilder::default().build();
        Self {
            header,
            uncles: Default::default(),
            short_ids: Default::default(),
            prefilled_transactions: Default::default(),
            proposals: Default::default(),
        }
    }
}

impl<'a> TryFrom<ckb_protocol::CompactBlock<'a>> for CompactBlock {
    type Error = FailureError;

    fn try_from(b: ckb_protocol::CompactBlock<'a>) -> Result<Self, Self::Error> {
        let header = cast!(b.header())?;
        let short_ids: Result<Vec<_>, FailureError> = cast!(b.short_ids())?
            .iter()
            .map(TryInto::try_into)
            .collect();
        let prefilled_transactions: Result<Vec<_>, FailureError> =
            FlatbuffersVectorIterator::new(cast!(b.prefilled_transactions())?)
                .map(TryInto::try_into)
                .collect();

        let uncles: Result<Vec<_>, FailureError> =
            FlatbuffersVectorIterator::new(cast!(b.uncles())?)
                .map(TryInto::try_into)
                .collect();
        let proposals: Result<Vec<_>, FailureError> = cast!(b.proposals())?
            .iter()
            .map(TryInto::try_into)
            .collect();

        Ok(CompactBlock {
            header: header.try_into()?,
            short_ids: short_ids?,
            prefilled_transactions: prefilled_transactions?,
            uncles: uncles?,
            proposals: proposals?,
        })
    }
}

impl CompactBlock {
    pub(crate) fn block_short_ids(&self) -> Vec<Option<ProposalShortId>> {
        let txs_len = self.prefilled_transactions.len() + self.short_ids.len();
        let mut block_short_ids: Vec<Option<ProposalShortId>> = Vec::with_capacity(txs_len);
        let prefilled_indexes = self
            .prefilled_transactions
            .iter()
            .map(|tx_index| tx_index.index)
            .collect::<HashSet<_>>();

        let mut index = 0;
        for i in 0..txs_len {
            if prefilled_indexes.contains(&i) {
                block_short_ids.push(None);
            } else {
                block_short_ids.push(self.short_ids.get(index).cloned());
                index += 1;
            }
        }
        block_short_ids
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct BlockProposal {
    pub transactions: Vec<Transaction>,
}

impl<'a> TryFrom<ckb_protocol::BlockProposal<'a>> for BlockProposal {
    type Error = FailureError;

    fn try_from(b: ckb_protocol::BlockProposal<'a>) -> Result<Self, Self::Error> {
        let transactions: Result<Vec<_>, FailureError> =
            FlatbuffersVectorIterator::new(cast!(b.transactions())?)
                .map(TryInto::try_into)
                .collect();

        Ok(BlockProposal {
            transactions: transactions?,
        })
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct GetBlockProposal {
    pub block_hash: H256,
    pub proposals: Vec<ProposalShortId>,
}

impl<'a> TryFrom<ckb_protocol::GetBlockProposal<'a>> for GetBlockProposal {
    type Error = FailureError;

    fn try_from(b: ckb_protocol::GetBlockProposal<'a>) -> Result<Self, Self::Error> {
        let block_hash = cast!(b.block_hash())?;
        let proposals: Result<Vec<_>, FailureError> = cast!(b.proposals())?
            .iter()
            .map(TryInto::try_into)
            .collect();

        Ok(GetBlockProposal {
            block_hash: block_hash.try_into()?,
            proposals: proposals?,
        })
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct TransactionHashes {
    pub hashes: Vec<H256>,
}

impl<'a> TryFrom<ckb_protocol::RelayTransactionHashes<'a>> for TransactionHashes {
    type Error = FailureError;

    fn try_from(b: ckb_protocol::RelayTransactionHashes<'a>) -> Result<Self, Self::Error> {
        let hashes: Result<Vec<_>, FailureError> = cast!(b.tx_hashes())?
            .iter()
            .map(TryInto::try_into)
            .collect();

        Ok(TransactionHashes { hashes: hashes? })
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct GetRelayTransactions {
    pub hashes: Vec<H256>,
}

impl<'a> TryFrom<ckb_protocol::GetRelayTransactions<'a>> for GetRelayTransactions {
    type Error = FailureError;

    fn try_from(b: ckb_protocol::GetRelayTransactions<'a>) -> Result<Self, Self::Error> {
        let hashes: Result<Vec<_>, FailureError> = cast!(b.tx_hashes())?
            .iter()
            .map(TryInto::try_into)
            .collect();

        Ok(GetRelayTransactions { hashes: hashes? })
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct RelayTransaction {
    pub cycles: Cycle,
    pub transaction: Transaction,
}

impl<'a> TryFrom<ckb_protocol::RelayTransaction<'a>> for RelayTransaction {
    type Error = FailureError;

    fn try_from(vtx: ckb_protocol::RelayTransaction<'a>) -> Result<Self, Self::Error> {
        let tx = cast!(vtx.transaction())?;
        let cycles = vtx.cycles();
        Ok(RelayTransaction {
            transaction: TryInto::try_into(tx)?,
            cycles,
        })
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct RelayTransactions {
    pub transactions: Vec<RelayTransaction>,
}

impl<'a> TryFrom<ckb_protocol::RelayTransactions<'a>> for RelayTransactions {
    type Error = FailureError;

    fn try_from(v: ckb_protocol::RelayTransactions<'a>) -> Result<Self, Self::Error> {
        let transactions: Vec<RelayTransaction> =
            FlatbuffersVectorIterator::new(cast!(v.transactions())?)
                .map(TryInto::try_into)
                .collect::<Result<_, FailureError>>()?;
        Ok(RelayTransactions { transactions })
    }
}
