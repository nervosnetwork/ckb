use crate::{BlockNumber, Capacity, Cycle, Timestamp, TransactionView, Uint64};
use ckb_types::H256;
use serde::{Deserialize, Serialize};

/// Transaction pool information.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct TxPoolInfo {
    /// The associated chain tip block hash.
    ///
    /// The transaction pool is stateful. It manages the transactions which are valid to be
    /// committed after this block.
    pub tip_hash: H256,
    /// The block number of the block `tip_hash`.
    pub tip_number: BlockNumber,
    /// Count of transactions in the pending state.
    ///
    /// The pending transactions must be proposed in a new block first.
    pub pending: Uint64,
    /// Count of transactions in the proposed state.
    ///
    /// The proposed transactions are ready to be committed in the new block after the block
    /// `tip_hash`.
    pub proposed: Uint64,
    /// Count of orphan transactions.
    ///
    /// An orphan transaction has an input cell from the transaction which is neither in the chain
    /// nor in the transaction pool.
    pub orphan: Uint64,
    /// Total count of transactions in the pool of all the different kinds of states.
    pub total_tx_size: Uint64,
    /// Total consumed VM cycles of all the transactions in the pool.
    pub total_tx_cycles: Uint64,
    /// Fee rate threshold. The pool rejects transactions which fee rate is below this threshold.
    ///
    /// The unit is Shannons per 1000 bytes transaction serialization size in the block.
    pub min_fee_rate: Uint64,
    /// Last updated time. This is the Unix timestamp in milliseconds.
    pub last_txs_updated_at: Timestamp,
}

/// The transaction entry in the pool.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct PoolTransactionEntry {
    /// The transaction.
    pub transaction: TransactionView,
    /// Consumed cycles.
    pub cycles: Cycle,
    /// The transaction serialized size in block.
    pub size: Uint64,
    /// The transaction fee.
    pub fee: Capacity,
}

/// Transaction output validators that prevent common mistakes.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum OutputsValidator {
    /// "default": The default validator which restricts the lock script and type script usage.
    ///
    /// The default validator only allows outputs (a.k.a., cells) that
    ///
    /// * use either the secp256k1 or the secp256k1 multisig bundled in the genesis block via type script hash as the lock script,
    /// * and the type script is either empty or DAO.
    Default,
    /// "passthrough": bypass the validator, thus allow any kind of transaction outputs.
    Passthrough,
}

impl OutputsValidator {
    /// TODO(doc): @doitian
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
