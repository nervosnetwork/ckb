use crate::{
    bytes::JsonBytes, BlockNumber, Cycle, EpochNumber, Header, ProposalShortId, Timestamp,
    Transaction, Unsigned, Version,
};
use ckb_core::block::BlockBuilder;
use ckb_core::header::HeaderBuilder;
use ckb_core::transaction::Transaction as CoreTransaction;
use ckb_core::uncle::UncleBlock as CoreUncleBlock;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use serde_derive::{Deserialize, Serialize};
use std::convert::From;

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct BlockTemplate {
    pub version: Version,
    pub difficulty: U256,
    pub current_time: Timestamp,
    pub number: BlockNumber,
    pub epoch: EpochNumber,
    pub parent_hash: H256,
    pub cycles_limit: Cycle,
    pub bytes_limit: Unsigned,
    pub uncles_count_limit: Unsigned,
    pub uncles: Vec<UncleTemplate>,
    pub transactions: Vec<TransactionTemplate>,
    pub proposals: Vec<ProposalShortId>,
    pub cellbase: CellbaseTemplate,
    pub work_id: Unsigned,
    pub dao: JsonBytes,
}

impl From<BlockTemplate> for BlockBuilder {
    fn from(block_template: BlockTemplate) -> BlockBuilder {
        let BlockTemplate {
            version,
            difficulty,
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

        let header_builder = HeaderBuilder::default()
            .version(version.0)
            .number(number.0)
            .epoch(epoch.0)
            .difficulty(difficulty)
            .timestamp(current_time.0)
            .parent_hash(parent_hash)
            .dao(dao.into_bytes());

        BlockBuilder::from_header_builder(header_builder)
            .uncles(uncles)
            .transaction(cellbase)
            .transactions(transactions)
            .proposals(proposals)
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct UncleTemplate {
    pub hash: H256,
    pub required: bool,
    pub proposals: Vec<ProposalShortId>,
    pub header: Header, // temporary
}

impl From<UncleTemplate> for CoreUncleBlock {
    fn from(template: UncleTemplate) -> Self {
        let UncleTemplate {
            proposals, header, ..
        } = template;

        CoreUncleBlock {
            header: header.into(),
            proposals: proposals
                .iter()
                .cloned()
                .map(Into::into)
                .collect::<Vec<_>>(),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct CellbaseTemplate {
    pub hash: H256,
    pub cycles: Option<Cycle>,
    pub data: Transaction, // temporary
}

impl From<CellbaseTemplate> for CoreTransaction {
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
    pub depends: Option<Vec<Unsigned>>,
    pub data: Transaction, // temporary
}

impl From<TransactionTemplate> for CoreTransaction {
    fn from(template: TransactionTemplate) -> Self {
        let TransactionTemplate { data, .. } = template;
        data.into()
    }
}
