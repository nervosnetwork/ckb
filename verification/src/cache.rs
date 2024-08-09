//! TX verification cache

use ckb_script::TransactionSnapshot;
use ckb_types::{
    core::{Capacity, Cycle, EntryCompleted},
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

/// TX verification lru entry
pub type CacheEntry = Completed;

/// Suspended state
#[derive(Clone, Debug)]
pub struct Suspended {
    /// Cached tx fee
    pub fee: Capacity,
    /// Snapshot
    pub snap: Arc<TransactionSnapshot>,
}

/// Completed entry
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completed {
    /// Cached tx cycles
    pub cycles: Cycle,
    /// Cached tx fee
    pub fee: Capacity,
}

impl From<Completed> for EntryCompleted {
    fn from(value: Completed) -> Self {
        EntryCompleted {
            cycles: value.cycles,
            fee: value.fee,
        }
    }
}
