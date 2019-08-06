//! The primary module containing the implementations of the transaction pool
//! and its top-level members.

use ckb_core::transaction::Transaction;
use ckb_core::Capacity;
use ckb_core::Cycle;
use serde_derive::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

/// Transaction pool configuration
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct TxPoolConfig {
    // Keep the transaction pool below <max_mem_size> mb
    pub max_mem_size: usize,
    // Keep the transaction pool below <max_cycles> cycles
    pub max_cycles: Cycle,
    // tx verify cache capacity
    pub max_verify_cache_size: usize,
    // conflict tx cache capacity
    pub max_conflict_cache_size: usize,
    // committed transactions hash cache capacity
    pub max_committed_txs_hash_cache_size: usize,
}

impl Default for TxPoolConfig {
    fn default() -> Self {
        TxPoolConfig {
            max_mem_size: 20_000_000, // 20mb
            max_cycles: 200_000_000_000,
            max_verify_cache_size: 100_000,
            max_conflict_cache_size: 1_000,
            max_committed_txs_hash_cache_size: 100_000,
        }
    }
}

/// An defect entry (conflict or orphan) in the transaction pool.
#[derive(Debug, Clone)]
pub struct DefectEntry {
    /// Transaction
    pub transaction: Transaction,
    /// refs count
    pub refs_count: usize,
    /// Cycles
    pub cycles: Option<Cycle>,
    /// tx size
    pub size: usize,
}

impl DefectEntry {
    /// Create new transaction pool entry
    pub fn new(
        tx: Transaction,
        refs_count: usize,
        cycles: Option<Cycle>,
        size: usize,
    ) -> DefectEntry {
        DefectEntry {
            transaction: tx,
            refs_count,
            cycles,
            size,
        }
    }
}

/// An entry in the transaction pool.
#[derive(Debug, Clone)]
pub struct PendingEntry {
    /// Transaction
    pub transaction: Transaction,
    /// Cycles
    pub cycles: Option<Cycle>,
    /// tx size
    pub size: usize,
}

impl PendingEntry {
    /// Create new transaction pool entry
    pub fn new(tx: Transaction, cycles: Option<Cycle>, size: usize) -> PendingEntry {
        PendingEntry {
            transaction: tx,
            cycles,
            size,
        }
    }
}

/// An entry in the transaction pool.
#[derive(Debug, Clone)]
pub struct ProposedEntry {
    /// Transaction
    pub transaction: Transaction,
    /// refs count
    pub refs_count: usize,
    /// Cycles
    pub cycles: Cycle,
    /// fee
    pub fee: Capacity,
    /// tx size
    pub size: usize,
}

impl ProposedEntry {
    /// Create new transaction pool entry
    pub fn new(
        tx: Transaction,
        refs_count: usize,
        cycles: Cycle,
        fee: Capacity,
        size: usize,
    ) -> ProposedEntry {
        ProposedEntry {
            transaction: tx,
            refs_count,
            cycles,
            fee,
            size,
        }
    }
}

impl Hash for ProposedEntry {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Hash::hash(&self.transaction, state);
    }
}

impl PartialEq for ProposedEntry {
    fn eq(&self, other: &ProposedEntry) -> bool {
        self.transaction == other.transaction
    }
}
