use serde::{Deserialize, Serialize};

/// TODO(doc): @doitian
#[derive(Copy, Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
pub struct Config {
    /// TODO(doc): @doitian
    pub header_cache_size: usize,
    /// TODO(doc): @doitian
    pub cell_data_cache_size: usize,
    /// TODO(doc): @doitian
    pub block_proposals_cache_size: usize,
    /// TODO(doc): @doitian
    pub block_tx_hashes_cache_size: usize,
    /// TODO(doc): @doitian
    pub block_uncles_cache_size: usize,
    /// TODO(doc): @doitian
    pub cellbase_cache_size: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            header_cache_size: 4096,
            cell_data_cache_size: 128,
            block_proposals_cache_size: 30,
            block_tx_hashes_cache_size: 30,
            block_uncles_cache_size: 30,
            cellbase_cache_size: 30,
        }
    }
}
