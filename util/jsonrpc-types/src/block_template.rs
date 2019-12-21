use crate::{
    BlockNumber, Byte32, Cycle, EpochNumberWithFraction, Header, ProposalShortId, Timestamp,
    Transaction, Uint32, Uint64, Version,
};
use ckb_types::{packed, prelude::*, H256};
use serde::{Deserialize, Serialize};
use std::convert::From;

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct BlockTemplate {
    pub version: Version,
    pub compact_target: Uint32,
    pub current_time: Timestamp,
    pub number: BlockNumber,
    pub epoch: EpochNumberWithFraction,
    pub parent_hash: H256,
    pub cycles_limit: Cycle,
    pub bytes_limit: Uint64,
    pub uncles_count_limit: Uint64,
    pub uncles: Vec<UncleTemplate>,
    pub transactions: Vec<TransactionTemplate>,
    pub proposals: Vec<ProposalShortId>,
    pub cellbase: CellbaseTemplate,
    pub work_id: Uint64,
    pub dao: Byte32,
}

impl From<BlockTemplate> for packed::Block {
    fn from(block_template: BlockTemplate) -> packed::Block {
        let BlockTemplate {
            version,
            compact_target,
            current_time,
            number,
            epoch,
            parent_hash,
            uncles,
            transactions,
            proposals,
            cellbase,
            dao,
            ..
        } = block_template;
        let raw = packed::RawHeader::new_builder()
            .version(version.pack())
            .compact_target(compact_target.pack())
            .parent_hash(parent_hash.pack())
            .timestamp(current_time.pack())
            .number(number.pack())
            .epoch(epoch.pack())
            .dao(dao.into())
            .build();
        let header = packed::Header::new_builder().raw(raw).build();
        let txs = packed::TransactionVec::new_builder()
            .push(cellbase.into())
            .extend(transactions.into_iter().map(|tx| tx.into()))
            .build();
        packed::Block::new_builder()
            .header(header)
            .uncles(
                uncles
                    .into_iter()
                    .map(|u| u.into())
                    .collect::<Vec<packed::UncleBlock>>()
                    .pack(),
            )
            .transactions(txs)
            .proposals(
                proposals
                    .into_iter()
                    .map(|p| p.into())
                    .collect::<Vec<packed::ProposalShortId>>()
                    .pack(),
            )
            .build()
            .reset_header()
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct UncleTemplate {
    pub hash: H256,
    pub required: bool,
    pub proposals: Vec<ProposalShortId>,
    pub header: Header, // temporary
}

impl From<UncleTemplate> for packed::UncleBlock {
    fn from(template: UncleTemplate) -> Self {
        let UncleTemplate {
            proposals, header, ..
        } = template;
        packed::UncleBlock::new_builder()
            .header(header.into())
            .proposals(proposals.into_iter().map(Into::into).pack())
            .build()
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct CellbaseTemplate {
    pub hash: H256,
    pub cycles: Option<Cycle>,
    pub data: Transaction, // temporary
}

impl From<CellbaseTemplate> for packed::Transaction {
    fn from(template: CellbaseTemplate) -> Self {
        let CellbaseTemplate { data, .. } = template;
        data.into()
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct TransactionTemplate {
    pub hash: H256,
    pub required: bool,
    pub cycles: Option<Cycle>,
    pub depends: Option<Vec<Uint64>>,
    pub data: Transaction, // temporary
}

impl From<TransactionTemplate> for packed::Transaction {
    fn from(template: TransactionTemplate) -> Self {
        let TransactionTemplate { data, .. } = template;
        data.into()
    }
}
