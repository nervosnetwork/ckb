//! The primary module containing the implementations of the transaction pool
//! and its top-level members.

use ckb_core::transaction::OutPoint;
use ckb_core::transaction::Transaction;
use ckb_core::Cycle;
use ckb_verification::TransactionError;
use failure::Fail;
use serde_derive::{Deserialize, Serialize};
use std::fmt;
use std::hash::{Hash, Hasher};

/// Transaction pool configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TxPoolConfig {
    /// Maximum capacity of the pool in number of transactions
    pub max_pool_size: usize,
    pub max_orphan_size: usize,
    pub max_proposal_size: usize,
    pub max_cache_size: usize,
    pub max_pending_size: usize,
    pub trace: Option<usize>,
}

impl Default for TxPoolConfig {
    fn default() -> Self {
        TxPoolConfig {
            max_pool_size: 10000,
            max_orphan_size: 10000,
            max_proposal_size: 10000,
            max_cache_size: 1000,
            max_pending_size: 10000,
            trace: Some(100),
        }
    }
}

impl TxPoolConfig {
    pub fn trace_enable(&self) -> bool {
        self.trace.is_some()
    }
}

// TODO document this enum more accurately
/// Enum of errors
#[derive(Debug, Clone, PartialEq, Fail)]
pub enum PoolError {
    /// An invalid pool entry caused by underlying tx validation error
    InvalidTx(TransactionError),
    /// CellStatus Conflict
    Conflict,
    /// Transaction pool is over capacity, can't accept more transactions
    OverCapacity,
    /// tx_pool don't accept cellbase-like tx
    NullInput,
    /// TimeOut
    TimeOut,
    /// BlockNumber is not right
    InvalidBlockNumber,
    /// Duplicate tx
    Duplicate,
    /// Tx contains unknown inputs
    UnknownInputs(Vec<OutPoint>),
}

impl fmt::Display for PoolError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self, f)
    }
}

/// An entry in the transaction pool.
#[derive(Debug, Clone)]
pub struct PoolEntry {
    /// Transaction
    pub transaction: Transaction,
    /// refs count
    pub refs_count: usize,
    /// Cycles
    pub cycles: Option<Cycle>,
}

impl PoolEntry {
    /// Create new transaction pool entry
    pub fn new(tx: Transaction, count: usize, cycles: Option<Cycle>) -> PoolEntry {
        PoolEntry {
            transaction: tx,
            refs_count: count,
            cycles,
        }
    }
}

impl Hash for PoolEntry {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Hash::hash(&self.transaction, state);
    }
}

impl PartialEq for PoolEntry {
    fn eq(&self, other: &PoolEntry) -> bool {
        self.transaction == other.transaction
    }
}
