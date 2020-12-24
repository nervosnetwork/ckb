use crate::{BlockNumber, Capacity, Cycle, Timestamp, TransactionView, Uint64};
use ckb_types::core::service::PoolTransactionEntry as CorePoolTransactionEntry;
use ckb_types::core::tx_pool::{Reject, TxEntryInfo, TxPoolEntryInfo, TxPoolIds as CoreTxPoolIds};
use ckb_types::prelude::Unpack;
use ckb_types::H256;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

impl From<CorePoolTransactionEntry> for PoolTransactionEntry {
    fn from(entry: CorePoolTransactionEntry) -> Self {
        PoolTransactionEntry {
            transaction: entry.transaction.into(),
            cycles: entry.cycles.into(),
            size: (entry.size as u64).into(),
            fee: entry.fee.into(),
        }
    }
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

/// Array of transaction ids
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct TxPoolIds {
    /// Pending transaction ids
    pub pending: Vec<H256>,
    /// Proposed transaction ids
    pub proposed: Vec<H256>,
}

impl From<CoreTxPoolIds> for TxPoolIds {
    fn from(ids: CoreTxPoolIds) -> Self {
        let CoreTxPoolIds { pending, proposed } = ids;
        TxPoolIds {
            pending: pending.iter().map(Unpack::unpack).collect(),
            proposed: proposed.iter().map(Unpack::unpack).collect(),
        }
    }
}

/// Transaction verbose info
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct TxVerbosity {
    /// Consumed cycles.
    pub cycles: Uint64,
    /// The transaction serialized size in block.
    pub size: Uint64,
    /// The transaction fee.
    pub fee: Capacity,
    /// Size of in-tx-pool ancestor transactions
    pub ancestors_size: Uint64,
    /// Cycles of in-tx-pool ancestor transactions
    pub ancestors_cycles: Uint64,
    /// Number of in-tx-pool ancestor transactions
    pub ancestors_count: Uint64,
}

impl From<TxEntryInfo> for TxVerbosity {
    fn from(info: TxEntryInfo) -> Self {
        TxVerbosity {
            cycles: info.cycles.into(),
            size: info.size.into(),
            fee: info.fee.into(),
            ancestors_size: info.ancestors_size.into(),
            ancestors_cycles: info.ancestors_cycles.into(),
            ancestors_count: info.ancestors_count.into(),
        }
    }
}

/// Tx-pool verbose object
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
pub struct TxPoolVerbosity {
    /// Pending tx verbose info
    pub pending: HashMap<H256, TxVerbosity>,
    /// Proposed tx verbose info
    pub proposed: HashMap<H256, TxVerbosity>,
}

impl From<TxPoolEntryInfo> for TxPoolVerbosity {
    fn from(info: TxPoolEntryInfo) -> Self {
        let TxPoolEntryInfo { pending, proposed } = info;

        TxPoolVerbosity {
            pending: pending
                .into_iter()
                .map(|(hash, entry)| (hash.unpack(), entry.into()))
                .collect(),
            proposed: proposed
                .into_iter()
                .map(|(hash, entry)| (hash.unpack(), entry.into()))
                .collect(),
        }
    }
}

/// All transactions in tx-pool.
///
/// `RawTxPool` is equivalent to [`TxPoolIds`][] `|` [`TxPoolVerbosity`][].
///
/// [`TxPoolIds`]: struct.TxPoolIds.html
/// [`TxPoolVerbosity`]: struct.TxPoolVerbosity.html
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(untagged)]
pub enum RawTxPool {
    /// verbose = false
    Ids(TxPoolIds),
    /// verbose = true
    Verbose(TxPoolVerbosity),
}

/// TX reject message
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "description")]
pub enum PoolTransactionReject {
    /// Transaction fee lower than config
    LowFeeRate(String),

    /// Transaction exceeded maximum ancestors count limit
    ExceededMaximumAncestorsCount(String),

    /// Transaction pool exceeded maximum size or cycles limit,
    Full(String),

    /// Transaction already exist in transaction_pool
    Duplicated(String),

    /// Malformed transaction
    Malformed(String),

    /// Resolve failed
    Resolve(String),

    /// Verification failed
    Verification(String),
}

impl From<Reject> for PoolTransactionReject {
    fn from(reject: Reject) -> Self {
        match reject {
            Reject::LowFeeRate(..) => Self::LowFeeRate(format!("{}", reject)),
            Reject::ExceededMaximumAncestorsCount => {
                Self::ExceededMaximumAncestorsCount(format!("{}", reject))
            }
            Reject::Full(..) => Self::Full(format!("{}", reject)),
            Reject::Duplicated(_) => Self::Duplicated(format!("{}", reject)),
            Reject::Malformed(_) => Self::Malformed(format!("{}", reject)),
            Reject::Resolve(_) => Self::Resolve(format!("{}", reject)),
            Reject::Verification(_) => Self::Verification(format!("{}", reject)),
        }
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
