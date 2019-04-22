use crate::proposal_short_id::ProposalShortId;
use crate::{Header, Transaction};
use ckb_core::{BlockNumber, Cycle, Version};
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use serde_derive::{Deserialize, Serialize};

use ckb_core::transaction::Transaction as CoreTransaction;
use ckb_core::uncle::UncleBlock as CoreUncleBlock;

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct BlockTemplate {
    pub version: Version,
    pub difficulty: U256,
    pub current_time: u64,
    pub number: BlockNumber,
    pub parent_hash: H256,
    pub cycles_limit: Cycle,
    pub bytes_limit: u64,
    pub uncles_count_limit: u32,
    pub uncles: Vec<UncleTemplate>,
    pub commit_transactions: Vec<TransactionTemplate>,
    pub proposal_transactions: Vec<ProposalShortId>,
    pub cellbase: CellbaseTemplate,
    pub work_id: String,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct UncleTemplate {
    pub hash: H256,
    pub required: bool,
    pub proposal_transactions: Vec<ProposalShortId>,
    pub header: Header, // temporary
}

impl From<UncleTemplate> for CoreUncleBlock {
    fn from(template: UncleTemplate) -> CoreUncleBlock {
        let UncleTemplate {
            proposal_transactions,
            header,
            ..
        } = template;

        CoreUncleBlock {
            header: header.into(),
            proposal_transactions: proposal_transactions
                .iter()
                .cloned()
                .map(Into::into)
                .collect(),
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
    fn from(template: CellbaseTemplate) -> CoreTransaction {
        let CellbaseTemplate { data, .. } = template;
        data.into()
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct TransactionTemplate {
    pub hash: H256,
    pub required: bool,
    pub cycles: Option<Cycle>,
    pub depends: Option<Vec<u32>>,
    pub data: Transaction, // temporary
}

impl From<TransactionTemplate> for CoreTransaction {
    fn from(template: TransactionTemplate) -> CoreTransaction {
        let TransactionTemplate { data, .. } = template;
        data.into()
    }
}
