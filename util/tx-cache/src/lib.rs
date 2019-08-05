use ckb_types::{
    core::{Capacity, Cycle},
    packed::Byte32,
};

pub type TxCache = lru_cache::LruCache<Byte32, TxCacheItem>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TxCacheItem {
    pub cycles: Cycle,
    pub fee: Capacity,
}

impl TxCacheItem {
    pub fn new(cycles: Cycle, fee: Capacity) -> Self {
        TxCacheItem { cycles, fee }
    }
}
