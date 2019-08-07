use std::convert::{TryFrom, TryInto};

use numext_fixed_hash::H256;

use ckb_core::{
    extras::{EpochExt, TransactionInfo},
    header::Header,
    transaction::{CellOutput, ProposalShortId, Transaction},
    uncle::UncleBlock,
    Bytes,
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

impl<'a> TryFrom<protos::StoredTransaction<'a>> for Transaction {
    type Error = Error;
    fn try_from(proto: protos::StoredTransaction<'a>) -> Result<Self> {
        let transaction = proto.data().unwrap_some()?;
        let hash = proto.cache().unwrap_some()?.hash().unwrap_some()?;
        let witness_hash = proto.cache().unwrap_some()?.witness_hash().unwrap_some()?;
        transaction.build_unchecked(hash.try_into()?, witness_hash.try_into()?)
    }
}

impl<'a> protos::StoredTransaction<'a> {
    pub fn hash(&self) -> Result<H256> {
        self.cache().unwrap_some()?.hash().unwrap_some()?.try_into()
    }

    pub fn witness_hash(&self) -> Result<H256> {
        self.cache()
            .unwrap_some()?
            .witness_hash()
            .unwrap_some()?
            .try_into()
    }

    pub fn cell_output(&self, output_index: usize) -> Result<Option<CellOutput>> {
        let outputs = self.data().unwrap_some()?.outputs().unwrap_some()?;
        let ret = if outputs.len() <= output_index {
            None
        } else {
            Some(outputs.get(output_index).try_into()?)
        };
        Ok(ret)
    }

    pub fn output_data(&self, output_index: usize) -> Result<Option<Bytes>> {
        let outputs_data = self.data().unwrap_some()?.outputs_data().unwrap_some()?;
        let ret = if outputs_data.len() <= output_index {
            None
        } else {
            Some(outputs_data.get(output_index).seq().unwrap_some()?.into())
        };
        Ok(ret)
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
