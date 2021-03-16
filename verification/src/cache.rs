//! TX verification cache

use ckb_types::{
    core::{Capacity, Cycle},
    packed::Byte32,
};
use moka::future::Cache;

/// TX verification lru cache
pub type TxVerifyCache = Cache<Byte32, CacheEntry>;

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
