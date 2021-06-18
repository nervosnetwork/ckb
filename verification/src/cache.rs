//! TX verification cache

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

/// TX verification lru entry
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CacheEntry {
    /// Cached tx cycles
    pub cycles: Cycle,
    /// Cached tx fee
    pub fee: Capacity,
}

impl CacheEntry {
    /// Constructs a CacheEntry
    pub fn new(cycles: Cycle, fee: Capacity) -> Self {
        CacheEntry { cycles, fee }
    }
}
