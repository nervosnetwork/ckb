use ckb_types::{
    core::{Capacity, Cycle},
    packed::Byte32,
};

pub type TxVerifyCache = lru::LruCache<Byte32, CacheEntry>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CacheEntry {
    pub cycles: Cycle,
    pub fee: Capacity,
}

impl CacheEntry {
    pub fn new(cycles: Cycle, fee: Capacity) -> Self {
        CacheEntry { cycles, fee }
    }
}
