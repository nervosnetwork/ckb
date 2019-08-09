use crate::{CELL_DATA_CACHE, HEADER_CACHE};
use lru_cache::LruCache;
use serde_derive::{Deserialize, Serialize};

#[derive(Copy, Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
pub struct StoreConfig {
    pub header_cache_size: usize,
    pub cell_data_cache_size: usize,
}

impl StoreConfig {
    pub fn apply(self) {
        *HEADER_CACHE.lock() = LruCache::new(self.header_cache_size);
        *CELL_DATA_CACHE.lock() = LruCache::new(self.cell_data_cache_size);
    }
}
