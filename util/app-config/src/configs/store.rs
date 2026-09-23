use serde::Serialize;

// The default values are set in the legacy version.
/// Store config options.
#[derive(Copy, Clone, Serialize, Eq, PartialEq, Hash, Debug)]
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
    pub block_extensions_cache_size: usize,
    /// Enable historical block archiving. Once enabled for a database, this must
    /// remain true; disabling it is rejected even before the first block is archived.
    /// The current epoch and the preceding 100 complete epochs remain hot.
    pub freezer_enable: bool,
}
