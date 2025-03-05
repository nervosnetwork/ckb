use crate::{BlockNumber, Capacity, Cycle, Timestamp, TransactionView, Uint64};
use ckb_types::H256;
use ckb_types::core::service::PoolTransactionEntry as CorePoolTransactionEntry;
use ckb_types::core::tx_pool::{
    AncestorsScoreSortKey as CoreAncestorsScoreSortKey, PoolTxDetailInfo as CorePoolTxDetailInfo,
    Reject, TxEntryInfo, TxPoolEntryInfo, TxPoolIds as CoreTxPoolIds, TxPoolInfo as CoreTxPoolInfo,
};
use ckb_types::prelude::Unpack;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Transaction pool information.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug, JsonSchema)]
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
    /// Total count of transactions in the pool of all the different kinds of states (excluding orphan transactions).
    pub total_tx_size: Uint64,
    /// Total consumed VM cycles of all the transactions in the pool (excluding orphan transactions).
    pub total_tx_cycles: Uint64,
    /// Fee rate threshold. The pool rejects transactions which fee rate is below this threshold.
    ///
    /// The unit is Shannons per 1000 bytes transaction serialization size in the block.
    pub min_fee_rate: Uint64,
    /// RBF rate threshold.
    ///
    /// The pool reject to replace for transactions which fee rate is below this threshold.
    /// if min_rbf_rate > min_fee_rate then RBF is enabled on the node.
    ///
    /// The unit is Shannons per 1000 bytes transaction serialization size in the block.
    pub min_rbf_rate: Uint64,
    /// Last updated time. This is the Unix timestamp in milliseconds.
    pub last_txs_updated_at: Timestamp,
    /// Limiting transactions to tx_size_limit
    ///
    /// Transactions with a large size close to the block size limit may not be packaged,
    /// because the block header and cellbase are occupied,
    /// so the tx-pool is limited to accepting transaction up to tx_size_limit.
    pub tx_size_limit: Uint64,
    /// Total limit on the size of transactions in the tx-pool
    pub max_tx_pool_size: Uint64,

    /// verify_queue size
    pub verify_queue_size: Uint64,
}

impl From<CoreTxPoolInfo> for TxPoolInfo {
    fn from(tx_pool_info: CoreTxPoolInfo) -> Self {
        TxPoolInfo {
            tip_hash: tx_pool_info.tip_hash.unpack(),
            tip_number: tx_pool_info.tip_number.into(),
            pending: (tx_pool_info.pending_size as u64).into(),
            proposed: (tx_pool_info.proposed_size as u64).into(),
            orphan: (tx_pool_info.orphan_size as u64).into(),
            total_tx_size: (tx_pool_info.total_tx_size as u64).into(),
            total_tx_cycles: tx_pool_info.total_tx_cycles.into(),
            min_fee_rate: tx_pool_info.min_fee_rate.as_u64().into(),
            min_rbf_rate: tx_pool_info.min_rbf_rate.as_u64().into(),
            last_txs_updated_at: tx_pool_info.last_txs_updated_at.into(),
            tx_size_limit: tx_pool_info.tx_size_limit.into(),
            max_tx_pool_size: tx_pool_info.max_tx_pool_size.into(),
            verify_queue_size: (tx_pool_info.verify_queue_size as u64).into(),
        }
    }
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
    /// The unix timestamp when entering the Txpool, unit: Millisecond
    pub timestamp: Uint64,
}

impl From<CorePoolTransactionEntry> for PoolTransactionEntry {
    fn from(entry: CorePoolTransactionEntry) -> Self {
        PoolTransactionEntry {
            transaction: entry.transaction.into(),
            cycles: entry.cycles.into(),
            size: (entry.size as u64).into(),
            fee: entry.fee.into(),
            timestamp: entry.timestamp.into(),
        }
    }
}

/// Transaction output validators that prevent common mistakes.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutputsValidator {
    /// the default validator, bypass output checking, thus allow any kind of transaction outputs.
    Passthrough,
    /// restricts the lock script and type script usage, see more information on <https://github.com/nervosnetwork/ckb/wiki/Transaction-%C2%BB-Default-Outputs-Validator>
    WellKnownScriptsOnly,
}

impl OutputsValidator {
    /// Gets the name of the validator when it is serialized into JSON string.
    pub fn json_display(&self) -> String {
        let v = serde_json::to_value(self).expect("OutputsValidator to JSON should never fail");
        v.as_str().unwrap_or_default().to_string()
    }
}

/// Array of transaction ids
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug, JsonSchema)]
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

/// Transaction entry info
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug, JsonSchema)]
pub struct TxPoolEntry {
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
    /// The unix timestamp when entering the Txpool, unit: Millisecond
    pub timestamp: Uint64,
}

impl From<TxEntryInfo> for TxPoolEntry {
    fn from(info: TxEntryInfo) -> Self {
        TxPoolEntry {
            cycles: info.cycles.into(),
            size: info.size.into(),
            fee: info.fee.into(),
            ancestors_size: info.ancestors_size.into(),
            ancestors_cycles: info.ancestors_cycles.into(),
            ancestors_count: info.ancestors_count.into(),
            timestamp: info.timestamp.into(),
        }
    }
}

/// Tx-pool entries object
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug, JsonSchema)]
pub struct TxPoolEntries {
    /// Pending tx verbose info
    pub pending: HashMap<H256, TxPoolEntry>,
    /// Proposed tx verbose info
    pub proposed: HashMap<H256, TxPoolEntry>,
    /// Conflicted tx hash vec
    pub conflicted: Vec<H256>,
}

impl From<TxPoolEntryInfo> for TxPoolEntries {
    fn from(info: TxPoolEntryInfo) -> Self {
        let TxPoolEntryInfo {
            pending,
            proposed,
            conflicted,
        } = info;

        TxPoolEntries {
            pending: pending
                .into_iter()
                .map(|(hash, entry)| (hash.unpack(), entry.into()))
                .collect(),
            proposed: proposed
                .into_iter()
                .map(|(hash, entry)| (hash.unpack(), entry.into()))
                .collect(),
            conflicted: conflicted.iter().map(Unpack::unpack).collect(),
        }
    }
}

/// All transactions in tx-pool.
///
/// `RawTxPool` is equivalent to [`TxPoolIds`][] `|` [`TxPoolEntries`][].
///
/// [`TxPoolIds`]: struct.TxPoolIds.html
/// [`TxPoolEntries`]: struct.TxPoolEntries.html
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug, JsonSchema)]
#[serde(untagged)]
pub enum RawTxPool {
    /// verbose = false
    Ids(TxPoolIds),
    /// verbose = true
    Verbose(TxPoolEntries),
}

/// A struct as a sorted key for tx-pool
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug, JsonSchema)]
pub struct AncestorsScoreSortKey {
    /// Fee
    pub fee: Uint64,
    /// Weight
    pub weight: Uint64,
    /// Ancestors fee
    pub ancestors_fee: Uint64,
    /// Ancestors weight
    pub ancestors_weight: Uint64,
}

impl From<CoreAncestorsScoreSortKey> for AncestorsScoreSortKey {
    fn from(value: CoreAncestorsScoreSortKey) -> Self {
        Self {
            fee: value.fee.into(),
            weight: value.weight.into(),
            ancestors_fee: value.ancestors_fee.into(),
            ancestors_weight: value.ancestors_weight.into(),
        }
    }
}

/// A Tx details info in tx-pool.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug, JsonSchema)]
pub struct PoolTxDetailInfo {
    /// The time added into tx-pool
    pub timestamp: Uint64,
    /// The detailed status in tx-pool, `pending`, `gap`, `proposed`
    pub entry_status: String,
    /// The rank in pending, starting from 0
    pub rank_in_pending: Uint64,
    /// The pending(`pending` and `gap`) count
    pub pending_count: Uint64,
    /// The proposed count
    pub proposed_count: Uint64,
    /// The descendants count of tx
    pub descendants_count: Uint64,
    /// The ancestors count of tx
    pub ancestors_count: Uint64,
    /// The score key details, useful to debug
    pub score_sortkey: AncestorsScoreSortKey,
}

impl From<CorePoolTxDetailInfo> for PoolTxDetailInfo {
    fn from(info: CorePoolTxDetailInfo) -> Self {
        Self {
            timestamp: info.timestamp.into(),
            entry_status: info.entry_status,
            rank_in_pending: (info.rank_in_pending as u64).into(),
            pending_count: (info.pending_count as u64).into(),
            proposed_count: (info.proposed_count as u64).into(),
            descendants_count: (info.descendants_count as u64).into(),
            ancestors_count: (info.ancestors_count as u64).into(),
            score_sortkey: info.score_sortkey.into(),
        }
    }
}

/// TX reject message, `PoolTransactionReject` is a JSON object with following fields.
///    * `type`:  the Reject type with following enum values
///    * `description`: `string` - Detailed description about why the transaction is rejected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "description")]
pub enum PoolTransactionReject {
    /// Transaction fee lower than config
    LowFeeRate(String),

    /// Transaction exceeded maximum ancestors count limit
    ExceededMaximumAncestorsCount(String),

    /// Transaction exceeded maximum size limit
    ExceededTransactionSizeLimit(String),

    /// Transaction are replaced because the pool is full
    Full(String),

    /// Transaction already exists in transaction_pool
    Duplicated(String),

    /// Malformed transaction
    Malformed(String),

    /// Declared wrong cycles
    DeclaredWrongCycles(String),

    /// Resolve failed
    Resolve(String),

    /// Verification failed
    Verification(String),

    /// Transaction expired
    Expiry(String),

    /// RBF rejected
    RBFRejected(String),

    /// Invalidated rejected
    Invalidated(String),
}

impl From<Reject> for PoolTransactionReject {
    fn from(reject: Reject) -> Self {
        match reject {
            Reject::LowFeeRate(..) => Self::LowFeeRate(format!("{reject}")),
            Reject::ExceededMaximumAncestorsCount => {
                Self::ExceededMaximumAncestorsCount(format!("{reject}"))
            }
            Reject::ExceededTransactionSizeLimit(..) => {
                Self::ExceededTransactionSizeLimit(format!("{reject}"))
            }
            Reject::Full(..) => Self::Full(format!("{reject}")),
            Reject::Duplicated(_) => Self::Duplicated(format!("{reject}")),
            Reject::Malformed(_, _) => Self::Malformed(format!("{reject}")),
            Reject::DeclaredWrongCycles(..) => Self::DeclaredWrongCycles(format!("{reject}")),
            Reject::Resolve(_) => Self::Resolve(format!("{reject}")),
            Reject::Verification(_) => Self::Verification(format!("{reject}")),
            Reject::Expiry(_) => Self::Expiry(format!("{reject}")),
            Reject::RBFRejected(_) => Self::RBFRejected(format!("{reject}")),
            Reject::Invalidated(_) => Self::Invalidated(format!("{reject}")),
        }
    }
}

/// Transaction's verify result by test_tx_pool_accept
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug, JsonSchema)]
pub struct EntryCompleted {
    /// Cached tx cycles
    pub cycles: Cycle,
    /// Cached tx fee
    pub fee: Capacity,
}

impl From<ckb_types::core::EntryCompleted> for EntryCompleted {
    fn from(value: ckb_types::core::EntryCompleted) -> Self {
        Self {
            cycles: value.cycles.into(),
            fee: value.fee.into(),
        }
    }
}
