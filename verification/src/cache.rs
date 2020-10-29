//! TODO(doc): @zhangsoledad
use ckb_types::{
    core::{Capacity, Cycle},
    packed::Byte32,
};

/// TODO(doc): @zhangsoledad
pub type TxVerifyCache = lru::LruCache<Byte32, CacheEntry>;

/// TODO(doc): @zhangsoledad
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CacheEntry {
    /// TODO(doc): @zhangsoledad
    pub cycles: Cycle,
    /// TODO(doc): @zhangsoledad
    pub fee: Capacity,
}

impl CacheEntry {
    /// TODO(doc): @zhangsoledad
    pub fn new(cycles: Cycle, fee: Capacity) -> Self {
        CacheEntry { cycles, fee }
    }
}
