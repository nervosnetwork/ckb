use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
pub struct Config {
    pub header_cache_size: usize,
    pub cell_data_cache_size: usize,
    pub block_proposals_cache_size: usize,
    pub block_tx_hashes_cache_size: usize,
    pub block_uncles_cache_size: usize,
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
