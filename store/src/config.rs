use crate::{
    BLOCK_PROPOSALS_CACHE, BLOCK_TX_HASHES_CACHE, BLOCK_UNCLES_CACHE, CELLBASE_CACHE,
    CELL_DATA_CACHE, HEADER_CACHE,
};
use lru_cache::LruCache;
use serde_derive::{Deserialize, Serialize};

#[derive(Copy, Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
pub struct StoreConfig {
    pub header_cache_size: usize,
    pub cell_data_cache_size: usize,
    pub block_proposals_cache_size: usize,
    pub block_tx_hashes_cache_size: usize,
    pub block_uncles_cache_size: usize,
    pub cellbase_cache_size: usize,
}

impl StoreConfig {
    pub fn apply(self) {
        *HEADER_CACHE.lock() = LruCache::new(self.header_cache_size);
        *CELL_DATA_CACHE.lock() = LruCache::new(self.cell_data_cache_size);
        *BLOCK_PROPOSALS_CACHE.lock() = LruCache::new(self.block_proposals_cache_size);
        *BLOCK_TX_HASHES_CACHE.lock() = LruCache::new(self.block_tx_hashes_cache_size);
        *BLOCK_UNCLES_CACHE.lock() = LruCache::new(self.block_uncles_cache_size);
        *CELLBASE_CACHE.lock() = LruCache::new(self.cellbase_cache_size);
    }
}
