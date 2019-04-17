use crate::{Cycle, Header, ProposalShortId, Transaction, Version};
use ckb_core::transaction::Transaction as CoreTransaction;
use ckb_core::uncle::UncleBlock as CoreUncleBlock;
use failure::Error as FailureError;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use serde_derive::{Deserialize, Serialize};
use std::convert::{TryFrom, TryInto};

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct BlockTemplate {
    pub version: Version,
    pub difficulty: U256,
    pub current_time: String,
    pub number: String,
    pub parent_hash: H256,
    pub cycles_limit: String,
    pub bytes_limit: String,
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

impl TryFrom<UncleTemplate> for CoreUncleBlock {
    type Error = FailureError;

    fn try_from(template: UncleTemplate) -> Result<Self, Self::Error> {
        let UncleTemplate {
            proposal_transactions,
            header,
            ..
        } = template;

        Ok(CoreUncleBlock {
            header: header.try_into()?,
            proposal_transactions: proposal_transactions
                .iter()
                .cloned()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct CellbaseTemplate {
    pub hash: H256,
    pub cycles: Option<Cycle>,
    pub data: Transaction, // temporary
}

impl TryFrom<CellbaseTemplate> for CoreTransaction {
    type Error = FailureError;

    fn try_from(template: CellbaseTemplate) -> Result<Self, Self::Error> {
        let CellbaseTemplate { data, .. } = template;
        data.try_into()
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

impl TryFrom<TransactionTemplate> for CoreTransaction {
    type Error = FailureError;

    fn try_from(template: TransactionTemplate) -> Result<Self, Self::Error> {
        let TransactionTemplate { data, .. } = template;
        data.try_into()
    }
}
