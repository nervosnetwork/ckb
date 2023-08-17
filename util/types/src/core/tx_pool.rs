//! Tx-pool shared type define.
use crate::core::{
    error::{OutPointError, TransactionError},
    BlockNumber, Capacity, Cycle, FeeRate,
};
use crate::packed::Byte32;
use crate::{core, H256};
use ckb_error::{
    impl_error_conversion_with_kind, prelude::*, Error, ErrorKind, InternalError, InternalErrorKind,
};
use std::collections::HashMap;

/// TX reject message
#[derive(Error, Debug, Clone)]
pub enum Reject {
    /// Transaction fee lower than config
    #[error("The min fee rate is {0}, so the transaction fee should be {1} shannons at least, but only got {2}")]
    LowFeeRate(FeeRate, u64, u64),

    /// Transaction exceeded maximum ancestors count limit
    #[error("Transaction exceeded maximum ancestors count limit, try send it later")]
    ExceededMaximumAncestorsCount,

    /// Transaction exceeded maximum size limit
    #[error("Transaction size {0} exceeded maximum limit {1}")]
    ExceededTransactionSizeLimit(u64, u64),

    /// Transaction are replaced because the pool is full
    #[error("Transaction are replaced because the pool is full, {0}")]
    Full(String),

    /// Transaction already exist in transaction_pool
    #[error("Transaction({0}) already exist in transaction_pool")]
    Duplicated(Byte32),

    /// Malformed transaction
    #[error("Malformed {0} transaction")]
    Malformed(String),

    /// Declared wrong cycles
    #[error("Declared wrong cycles {0}, actual {1}")]
    DeclaredWrongCycles(Cycle, Cycle),

    /// Resolve failed
    #[error("Resolve failed {0}")]
    Resolve(OutPointError),

    /// Verification failed
    #[error("Verification failed {0}")]
    Verification(Error),

    /// Expired
    #[error("Expiry transaction, timestamp {0}")]
    Expiry(u64),

    /// RBF rejected
    #[error("RBF rejected: {0}")]
    RBFRejected(String),
}

fn is_malformed_from_verification(error: &Error) -> bool {
    match error.kind() {
        ErrorKind::Transaction => error
            .downcast_ref::<TransactionError>()
            .expect("error kind checked")
            .is_malformed_tx(),
        ErrorKind::Script => true,
        ErrorKind::Internal => {
            error
                .downcast_ref::<InternalError>()
                .expect("error kind checked")
                .kind()
                == InternalErrorKind::CapacityOverflow
        }
        _ => false,
    }
}

impl Reject {
    /// Returns true if the reject reason is malformed tx.
    pub fn is_malformed_tx(&self) -> bool {
        match self {
            Reject::Malformed(_) => true,
            Reject::DeclaredWrongCycles(..) => true,
            Reject::Verification(err) => is_malformed_from_verification(err),
            Reject::Resolve(OutPointError::OverMaxDepExpansionLimit) => true,
            _ => false,
        }
    }

    /// Returns true if tx can be resubmitted, allowing relay
    /// * Declared wrong cycles should allow relay with the correct cycles
    /// * Reject but is not malformed and the fee rate reached the threshold,
    ///     it may be due to double spending
    ///     or temporary limitations of the pool resources,
    ///     and expired clearing
    pub fn is_allowed_relay(&self) -> bool {
        matches!(self, Reject::DeclaredWrongCycles(..))
            || (!matches!(self, Reject::LowFeeRate(..)) && !self.is_malformed_tx())
    }
}

impl_error_conversion_with_kind!(Reject, ErrorKind::SubmitTransaction, Error);

/// Tx-pool transaction status
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TxStatus {
    /// Status "pending". The transaction is in the pool, and not proposed yet.
    Pending,
    /// Status "proposed". The transaction is in the pool and has been proposed.
    Proposed,
    /// Status "committed". The transaction has been committed to the canonical chain.
    Committed(H256),
    /// Status "unknown". The node has not seen the transaction,
    /// or it should be rejected but was cleared due to storage limitations.
    Unknown,
    /// Status "rejected". The transaction has been recently removed from the pool.
    /// Due to storage limitations, the node can only hold the most recently removed transactions.
    Rejected(String),
}

/// Tx-pool entry info
#[derive(Debug, PartialEq, Eq)]
pub struct TxEntryInfo {
    /// Consumed cycles.
    pub cycles: Cycle,
    /// The transaction serialized size in block.
    pub size: u64,
    /// The transaction fee.
    pub fee: Capacity,
    /// Size of in-tx-pool ancestor transactions
    pub ancestors_size: u64,
    /// Cycles of in-tx-pool ancestor transactions
    pub ancestors_cycles: u64,
    /// Size of in-tx-pool descendants transactions
    pub descendants_size: u64,
    /// Cycles of in-tx-pool descendants transactions
    pub descendants_cycles: u64,
    /// Number of in-tx-pool ancestor transactions
    pub ancestors_count: u64,
    /// The unix timestamp when entering the Txpool, unit: Millisecond
    pub timestamp: u64,
}

/// Array of transaction ids
#[derive(Debug, PartialEq, Eq)]
pub struct TxPoolIds {
    /// Pending transaction ids
    pub pending: Vec<Byte32>,
    /// Proposed transaction ids
    pub proposed: Vec<Byte32>,
}

/// All in-pool transaction entry info
#[derive(Debug, PartialEq, Eq)]
pub struct TxPoolEntryInfo {
    /// Pending transaction entry info
    pub pending: HashMap<Byte32, TxEntryInfo>,
    /// Proposed transaction entry info
    pub proposed: HashMap<Byte32, TxEntryInfo>,
}

/// The JSON view of a transaction as well as its status.
#[derive(Clone, Debug)]
pub struct TransactionWithStatus {
    /// The transaction.
    pub transaction: Option<core::TransactionView>,
    /// The transaction status.
    pub tx_status: TxStatus,
    /// The transaction verification consumed cycles
    pub cycles: Option<core::Cycle>,
    /// The transaction fee of the transaction
    pub fee: Option<Capacity>,
    /// The minimal fee required to replace this transaction
    pub min_replace_fee: Option<Capacity>,
    /// If the transaction is in tx-pool, `time_added_to_pool` represent when it enter the tx-pool. unit: Millisecond
    pub time_added_to_pool: Option<u64>,
}

impl TransactionWithStatus {
    /// Build with tx status
    pub fn with_status(
        tx: Option<core::TransactionView>,
        cycles: core::Cycle,
        time_added_to_pool: u64,
        tx_status: TxStatus,
        fee: Option<Capacity>,
        min_replace_fee: Option<Capacity>,
    ) -> Self {
        Self {
            tx_status,
            fee,
            min_replace_fee,
            transaction: tx,
            cycles: Some(cycles),
            time_added_to_pool: Some(time_added_to_pool),
        }
    }

    /// Build with committed status
    pub fn with_committed(
        tx: Option<core::TransactionView>,
        hash: H256,
        cycles: Option<core::Cycle>,
        fee: Option<Capacity>,
    ) -> Self {
        Self {
            tx_status: TxStatus::Committed(hash),
            transaction: tx,
            cycles,
            fee,
            min_replace_fee: None,
            time_added_to_pool: None,
        }
    }

    /// Build with rejected status
    pub fn with_rejected(reason: String) -> Self {
        Self {
            tx_status: TxStatus::Rejected(reason),
            transaction: None,
            cycles: None,
            fee: None,
            min_replace_fee: None,
            time_added_to_pool: None,
        }
    }

    /// Build with rejected status
    pub fn with_unknown() -> Self {
        Self {
            tx_status: TxStatus::Unknown,
            transaction: None,
            cycles: None,
            fee: None,
            min_replace_fee: None,
            time_added_to_pool: None,
        }
    }

    /// Omit transaction
    pub fn omit_transaction(tx_status: TxStatus, cycles: Option<core::Cycle>) -> Self {
        Self {
            tx_status,
            transaction: None,
            cycles,
            fee: None,
            min_replace_fee: None,
            time_added_to_pool: None,
        }
    }

    /// Returns true if the tx_status is Unknown.
    pub fn is_unknown(&self) -> bool {
        matches!(self.tx_status, TxStatus::Unknown)
    }
}

/// Equal to MAX_BLOCK_BYTES / MAX_BLOCK_CYCLES, see ckb-chain-spec.
/// The precision is set so that the difference between MAX_BLOCK_CYCLES * DEFAULT_BYTES_PER_CYCLES
/// and MAX_BLOCK_BYTES is less than 1.
pub const DEFAULT_BYTES_PER_CYCLES: f64 = 0.000_170_571_4_f64;

/// vbytes has been deprecated, renamed to weight to prevent ambiguity
#[deprecated(
    since = "0.107.0",
    note = "Please use the get_transaction_weight instead"
)]
pub fn get_transaction_virtual_bytes(tx_size: usize, cycles: u64) -> u64 {
    std::cmp::max(
        tx_size as u64,
        (cycles as f64 * DEFAULT_BYTES_PER_CYCLES) as u64,
    )
}

/// The miners select transactions to fill the limited block space which gives the highest fee.
/// Because there are two different limits, serialized size and consumed cycles,
/// the selection algorithm is a multi-dimensional knapsack problem.
/// Introducing the transaction weight converts the multi-dimensional knapsack to a typical knapsack problem,
/// which has a simple greedy algorithm.
pub fn get_transaction_weight(tx_size: usize, cycles: u64) -> u64 {
    std::cmp::max(
        tx_size as u64,
        (cycles as f64 * DEFAULT_BYTES_PER_CYCLES) as u64,
    )
}

/// The maximum size of the tx-pool to accept transactions
/// The ckb consensus does not limit the size of a single transaction,
/// but if the size of the transaction is close to the limit of the block,
/// it may cause the transaction to fail to be packed
pub const TRANSACTION_SIZE_LIMIT: u64 = 512 * 1_000;

/// Transaction pool information.
#[derive(Clone, Debug)]
pub struct TxPoolInfo {
    /// The associated chain tip block hash.
    ///
    /// Transaction pool is stateful. It manages the transactions which are valid to be commit
    /// after this block.
    pub tip_hash: Byte32,
    /// The block number of the block `tip_hash`.
    pub tip_number: BlockNumber,
    /// Count of transactions in the pending state.
    ///
    /// The pending transactions must be proposed in a new block first.
    pub pending_size: usize,
    /// Count of transactions in the proposed state.
    ///
    /// The proposed transactions are ready to be commit in the new block after the block
    /// `tip_hash`.
    pub proposed_size: usize,
    /// Count of orphan transactions.
    ///
    /// An orphan transaction has an input cell from the transaction which is neither in the chain
    /// nor in the transaction pool.
    pub orphan_size: usize,
    /// Total count of transactions in the pool of all the different kinds of states.
    pub total_tx_size: usize,
    /// Total consumed VM cycles of all the transactions in the pool.
    pub total_tx_cycles: Cycle,
    /// Fee rate threshold. The pool rejects transactions which fee rate is below this threshold.
    ///
    /// The unit is Shannons per 1000 bytes transaction serialization size in the block.
    pub min_fee_rate: FeeRate,

    /// Min RBF rate threshold. The pool reject RBF transactions which fee rate is below this threshold.
    /// if min_rbf_rate > min_fee_rate then RBF is enabled on the node.
    ///
    /// The unit is Shannons per 1000 bytes transaction serialization size in the block.
    pub min_rbf_rate: FeeRate,

    /// Last updated time. This is the Unix timestamp in milliseconds.
    pub last_txs_updated_at: u64,
    /// Limiting transactions to tx_size_limit
    ///
    /// Transactions with a large size close to the block size limit may not be packaged,
    /// because the block header and cellbase are occupied,
    /// so the tx-pool is limited to accepting transaction up to tx_size_limit.
    pub tx_size_limit: u64,
    /// Total limit on the size of transactions in the tx-pool
    pub max_tx_pool_size: u64,
}
