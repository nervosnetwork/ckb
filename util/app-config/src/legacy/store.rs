use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoreConfig {
    header_cache_size: usize,
    cell_data_cache_size: usize,
    block_proposals_cache_size: usize,
    block_tx_hashes_cache_size: usize,
    block_uncles_cache_size: usize,
    pub(crate) cellbase_cache_size: Option<usize>,
    #[serde(default = "default_block_extensions_cache_size")]
    block_extensions_cache_size: usize,
    #[serde(default = "default_freezer_enable")]
    freezer_enable: bool,
}

const fn default_block_extensions_cache_size() -> usize {
    30
}

const fn default_freezer_enable() -> bool {
    false
}

impl Default for crate::StoreConfig {
    fn default() -> Self {
        StoreConfig::default().into()
    }
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            header_cache_size: 4096,
            cell_data_cache_size: 128,
            block_proposals_cache_size: 30,
            block_tx_hashes_cache_size: 30,
            block_uncles_cache_size: 30,
            cellbase_cache_size: None,
            block_extensions_cache_size: default_block_extensions_cache_size(),
            freezer_enable: default_freezer_enable(),
        }
    }
}

impl From<StoreConfig> for crate::StoreConfig {
    fn from(input: StoreConfig) -> Self {
        let StoreConfig {
            header_cache_size,
            cell_data_cache_size,
            block_proposals_cache_size,
            block_tx_hashes_cache_size,
            block_uncles_cache_size,
            cellbase_cache_size: _,
            block_extensions_cache_size,
            freezer_enable,
        } = input;
        Self {
            header_cache_size,
            cell_data_cache_size,
            block_proposals_cache_size,
            block_tx_hashes_cache_size,
            block_uncles_cache_size,
            block_extensions_cache_size,
            freezer_enable,
        }
    }
}
