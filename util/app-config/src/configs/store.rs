use serde::{Deserialize, Serialize};

/// Store config options.
#[derive(Copy, Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
pub struct Config {
    /// The maximum number of cached block headers.
    pub header_cache_size: usize,
    /// The maximum number of cached cell data.
    pub cell_data_cache_size: usize,
    /// The maximum number of blocks which proposals section is cached.
    pub block_proposals_cache_size: usize,
    /// The maximum number of blocks which tx hashes are cached.
    pub block_tx_hashes_cache_size: usize,
    /// The maximum number of blocks which uncles section is cached.
    pub block_uncles_cache_size: usize,
    /// The maximum number of blocks which extension section is cached.
    #[serde(default = "default_block_extensions_cache_size")]
    pub block_extensions_cache_size: usize,
    /// The maximum number of blocks which cellbase transaction is cached.
    pub cellbase_cache_size: usize,
    /// whether enable freezer
    #[serde(default = "default_freezer_enable")]
    pub freezer_enable: bool,
}

const fn default_block_extensions_cache_size() -> usize {
    30
}

fn default_freezer_enable() -> bool {
    false
}

impl Default for Config {
    fn default() -> Self {
        Config {
            header_cache_size: 4096,
            cell_data_cache_size: 128,
            block_proposals_cache_size: 30,
            block_tx_hashes_cache_size: 30,
            block_uncles_cache_size: 30,
            block_extensions_cache_size: default_block_extensions_cache_size(),
            cellbase_cache_size: 30,
            freezer_enable: false,
        }
    }
}
