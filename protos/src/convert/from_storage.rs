use numext_fixed_hash::H256;

use ckb_core::{
    extras::{EpochExt, TransactionInfo},
    header::Header,
    transaction::{CellOutput, ProposalShortId, Transaction},
    uncle::UncleBlock,
};

use crate as protos;
use crate::convert::FbVecIntoIterator;

impl<'a> protos::StoredBlock<'a> {
    pub fn header(&self) -> Header {
        let header = cast!(cast!(self.data()).header());
        let hash = cast!(cast!(self.cache()).header_hash());
        header.build_unchecked(hash.into())
    }
}

impl<'a> From<protos::StoredBlockBody<'a>> for Vec<Transaction> {
    fn from(proto: protos::StoredBlockBody<'a>) -> Self {
        let transactions = cast!(cast!(proto.data()).transactions());
        let tx_hashes = cast!(cast!(proto.cache()).tx_hashes());
        let tx_witness_hashes = cast!(cast!(proto.cache()).tx_witness_hashes());
        transactions
            .iter()
            .zip(tx_hashes.iter())
            .zip(tx_witness_hashes.iter())
            .map(|((tx, hash), witness_hash)| tx.build_unchecked(hash.into(), witness_hash.into()))
            .collect()
    }
}

impl<'a> protos::StoredBlockBody<'a> {
    pub fn tx_hashes(&self) -> Vec<H256> {
        cast!(cast!(self.cache()).tx_hashes())
            .iter()
            .map(Into::into)
            .collect()
    }

    pub fn transaction(&self, index: usize) -> Option<Transaction> {
        let transactions = cast!(cast!(self.data()).transactions());
        if transactions.len() <= index {
            None
        } else {
            let tx_hashes = cast!(cast!(self.cache()).tx_hashes());
            let tx_witness_hashes = cast!(cast!(self.cache()).tx_witness_hashes());
            let tx = transactions.get(index).build_unchecked(
                cast!(tx_hashes.get(index)).into(),
                cast!(tx_witness_hashes.get(index)).into(),
            );
            Some(tx)
        }
    }

    pub fn output(&self, tx_index: usize, output_index: usize) -> Option<CellOutput> {
        let transactions = cast!(cast!(self.data()).transactions());
        if transactions.len() <= tx_index {
            None
        } else {
            let outputs = cast!(transactions.get(tx_index).outputs());
            if outputs.len() <= output_index {
                None
            } else {
                Some(outputs.get(output_index).into())
            }
        }
    }
}

impl<'a> From<protos::StoredTransactionInfo<'a>> for TransactionInfo {
    fn from(proto: protos::StoredTransactionInfo<'a>) -> Self {
        cast!(proto.data()).into()
    }
}

impl<'a> From<protos::StoredHeader<'a>> for Header {
    fn from(proto: protos::StoredHeader<'a>) -> Self {
        let header = cast!(proto.data());
        let hash = cast!(cast!(proto.cache()).hash());
        header.build_unchecked(hash.into())
    }
}

impl<'a> From<protos::StoredUncleBlocks<'a>> for Vec<UncleBlock> {
    fn from(proto: protos::StoredUncleBlocks<'a>) -> Self {
        let uncles = cast!(proto.data());
        let hashes = cast!(cast!(proto.cache()).hashes());
        uncles
            .iter()
            .zip(hashes.iter())
            .map(|(uncle, hash)| uncle.build_unchecked(hash.into()))
            .collect()
    }
}

impl<'a> From<protos::StoredProposalShortIds<'a>> for Vec<ProposalShortId> {
    fn from(proto: protos::StoredProposalShortIds<'a>) -> Self {
        let proposals = cast!(proto.data());
        proposals.iter().map(Into::into).collect()
    }
}

impl<'a> From<protos::StoredEpochExt<'a>> for EpochExt {
    fn from(proto: protos::StoredEpochExt<'a>) -> Self {
        cast!(proto.data()).into()
    }
}
