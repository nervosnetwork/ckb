//! TX verification cache

use ckb_script::TransactionSnapshot;
use ckb_types::{
    core::{Capacity, Cycle},
    packed::Byte32,
};
use std::sync::Arc;

/// TX verification lru cache
pub type TxVerificationCache = lru::LruCache<Byte32, CacheEntry>;

const CACHE_SIZE: usize = 1000 * 30;

/// Initialize cache
pub fn init_cache() -> TxVerificationCache {
    lru::LruCache::new(CACHE_SIZE)
}

#[derive(Clone, Debug)]
/// TX verification lru entry
pub enum CacheEntry {
    /// Completed
    Completed(Completed),
    /// Suspended
    Suspended(Suspended),
}

/// Suspended state
#[derive(Clone, Debug)]
pub struct Suspended {
    /// Cached tx fee
    pub fee: Capacity,
    /// Snapshot
    pub snap: Arc<TransactionSnapshot>,
}

/// Completed entry
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Completed {
    /// Cached tx cycles
    pub cycles: Cycle,
    /// Cached tx fee
    pub fee: Capacity,
}

impl CacheEntry {
    /// Constructs a completed CacheEntry
    pub fn completed(cycles: Cycle, fee: Capacity) -> Self {
        CacheEntry::Completed(Completed { cycles, fee })
    }

    /// Constructs a Suspended CacheEntry
    pub fn suspended(snap: Arc<TransactionSnapshot>, fee: Capacity) -> Self {
        CacheEntry::Suspended(Suspended { snap, fee })
    }
}
