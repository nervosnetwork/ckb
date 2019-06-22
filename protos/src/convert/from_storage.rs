use std::convert::{TryFrom, TryInto};

use numext_fixed_hash::H256;

use ckb_core::{
    extras::{EpochExt, TransactionInfo},
    header::Header,
    transaction::{CellOutput, ProposalShortId, Transaction},
    uncle::UncleBlock,
    Capacity,
};

use crate::convert::{FbVecIntoIterator, OptionShouldBeSome};
use crate::{
    self as protos,
    error::{Error, Result},
};

impl<'a> protos::StoredBlock<'a> {
    pub fn header(&self) -> Result<Header> {
        let header = self.data().unwrap_some()?.header().unwrap_some()?;
        let hash = self.cache().unwrap_some()?.header_hash().unwrap_some()?;
        header.build_unchecked(hash.try_into()?)
    }
}

impl<'a> TryFrom<protos::StoredBlockBody<'a>> for Vec<Transaction> {
    type Error = Error;
    fn try_from(proto: protos::StoredBlockBody<'a>) -> Result<Self> {
        let transactions = proto.data().unwrap_some()?.transactions().unwrap_some()?;
        let tx_hashes = proto.cache().unwrap_some()?.tx_hashes().unwrap_some()?;
        let tx_witness_hashes = proto
            .cache()
            .unwrap_some()?
            .tx_witness_hashes()
            .unwrap_some()?;
        transactions
            .iter()
            .zip(tx_hashes.iter())
            .zip(tx_witness_hashes.iter())
            .map(|((tx, hash), witness_hash)| {
                tx.build_unchecked(hash.try_into()?, witness_hash.try_into()?)
            })
            .collect()
    }
}

impl<'a> protos::StoredBlockBody<'a> {
    pub fn tx_hashes(&self) -> Result<Vec<H256>> {
        self.cache()
            .unwrap_some()?
            .tx_hashes()
            .unwrap_some()?
            .iter()
            .map(TryInto::try_into)
            .collect()
    }

    pub fn transaction(&self, index: usize) -> Result<Option<Transaction>> {
        let transactions = self.data().unwrap_some()?.transactions().unwrap_some()?;
        let ret = if transactions.len() <= index {
            None
        } else {
            let tx_hashes = self.cache().unwrap_some()?.tx_hashes().unwrap_some()?;
            let tx_witness_hashes = self
                .cache()
                .unwrap_some()?
                .tx_witness_hashes()
                .unwrap_some()?;
            let tx = transactions.get(index).build_unchecked(
                tx_hashes.get(index).unwrap_some()?.try_into()?,
                tx_witness_hashes.get(index).unwrap_some()?.try_into()?,
            )?;
            Some(tx)
        };
        Ok(ret)
    }

    pub fn output(&self, tx_index: usize, output_index: usize) -> Result<Option<CellOutput>> {
        let transactions = self.data().unwrap_some()?.transactions().unwrap_some()?;
        let ret = if transactions.len() <= tx_index {
            None
        } else {
            let outputs = transactions.get(tx_index).outputs().unwrap_some()?;
            if outputs.len() <= output_index {
                None
            } else {
                Some(outputs.get(output_index).try_into()?)
            }
        };
        Ok(ret)
    }
}

impl<'a> TryFrom<protos::StoredTransactionInfo<'a>> for TransactionInfo {
    type Error = Error;
    fn try_from(proto: protos::StoredTransactionInfo<'a>) -> Result<Self> {
        proto.data().unwrap_some()?.try_into()
    }
}

impl<'a> TryFrom<protos::StoredHeader<'a>> for Header {
    type Error = Error;
    fn try_from(proto: protos::StoredHeader<'a>) -> Result<Self> {
        let header = proto.data().unwrap_some()?;
        let hash = proto.cache().unwrap_some()?.hash().unwrap_some()?;
        header.build_unchecked(hash.try_into()?)
    }
}

impl<'a> TryFrom<protos::StoredUncleBlocks<'a>> for Vec<UncleBlock> {
    type Error = Error;
    fn try_from(proto: protos::StoredUncleBlocks<'a>) -> Result<Self> {
        let uncles = proto.data().unwrap_some()?;
        let hashes = proto.cache().unwrap_some()?.hashes().unwrap_some()?;
        uncles
            .iter()
            .zip(hashes.iter())
            .map(|(uncle, hash)| uncle.build_unchecked(hash.try_into()?))
            .collect()
    }
}

impl<'a> TryFrom<protos::StoredProposalShortIds<'a>> for Vec<ProposalShortId> {
    type Error = Error;
    fn try_from(proto: protos::StoredProposalShortIds<'a>) -> Result<Self> {
        let proposals = proto.data().unwrap_some()?;
        proposals.iter().map(TryInto::try_into).collect()
    }
}

impl<'a> TryFrom<protos::StoredEpochExt<'a>> for EpochExt {
    type Error = Error;
    fn try_from(proto: protos::StoredEpochExt<'a>) -> Result<Self> {
        proto.data().unwrap_some()?.try_into()
    }
}

impl<'a> TryFrom<protos::StoredCellMeta<'a>> for (Capacity, H256) {
    type Error = Error;
    fn try_from(proto: protos::StoredCellMeta<'a>) -> Result<Self> {
        proto.data().unwrap_some()?.try_into()
    }
}
