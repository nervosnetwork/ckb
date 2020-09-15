use crate::{BlockNumber, Timestamp, Uint64};
use ckb_types::{core, H256};
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct TxPoolInfo {
    pub tip_hash: H256,
    pub tip_number: BlockNumber,
    pub pending: Uint64,
    pub proposed: Uint64,
    pub orphan: Uint64,
    pub total_tx_size: Uint64,
    pub total_tx_cycles: Uint64,
    pub min_fee_rate: Uint64,
    pub last_txs_updated_at: Timestamp,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum OutputsValidator {
    Default,
    Passthrough,
}

impl OutputsValidator {
    pub fn json_display(&self) -> String {
        let v = serde_json::to_value(self).expect("OutputsValidator to JSON should never fail");
        v.as_str().unwrap_or_default().to_string()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub enum PoolKind {
    Pending,
    Proposed,
}

impl From<core::PoolKind> for PoolKind {
    fn from(input: core::PoolKind) -> PoolKind {
        match input {
            core::PoolKind::Pending => PoolKind::Pending,
            core::PoolKind::Proposed => PoolKind::Proposed,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct TxPoolEntry {
    pub pool: PoolKind,
    pub cycles: Uint64,
    pub size_in_block: Uint64,
    pub fees: Uint64,
    pub ancestors_size: Uint64,
    pub ancestors_fee: Uint64,
    pub ancestors_cycles: Uint64,
    pub ancestors_count: Uint64,
    pub witness_hash: H256,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct TxPoolIds {
    pending: Vec<H256>,
    proposed: Vec<H256>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct TxPoolVerbose {
    pending: Vec<TxPoolEntry>,
    proposed: Vec<TxPoolEntry>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub enum RawTxPool {
    Ids(TxPoolIds),
    Verbose(TxPoolVerbose),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_outputs_validator_json_display() {
        assert_eq!("default", OutputsValidator::Default.json_display());
        assert_eq!("passthrough", OutputsValidator::Passthrough.json_display());
    }
}
