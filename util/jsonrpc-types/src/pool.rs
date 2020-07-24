use crate::{BlockNumber, Timestamp, Uint64};
use ckb_types::H256;
use serde::{Deserialize, Serialize};
use serde_json;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_outputs_validator_json_display() {
        assert_eq!("default", OutputsValidator::Default.json_display());
        assert_eq!("passthrough", OutputsValidator::Passthrough.json_display());
    }
}
