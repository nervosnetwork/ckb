use ckb_app_config::StoreConfig;
use ckb_types::{
    bytes::Bytes,
    core::{HeaderView, UncleBlockVecView},
    packed::{self, Byte32, ProposalShortIdVec},
};
use ckb_util::Mutex;
use lru::LruCache;

/// TODO(doc): @quake
pub struct StoreCache {
    /// TODO(doc): @quake
    pub headers: Mutex<LruCache<Byte32, HeaderView>>,
    /// TODO(doc): @quake
    pub cell_data: Mutex<LruCache<Vec<u8>, (Bytes, Byte32)>>,
    /// TODO(doc): @quake
    pub cell_data_hash: Mutex<LruCache<Vec<u8>, Byte32>>,
    /// TODO(doc): @quake
    pub block_proposals: Mutex<LruCache<Byte32, ProposalShortIdVec>>,
    /// TODO(doc): @quake
    pub block_tx_hashes: Mutex<LruCache<Byte32, Vec<Byte32>>>,
    /// TODO(doc): @quake
    pub block_uncles: Mutex<LruCache<Byte32, UncleBlockVecView>>,
    /// The cache of block extension sections.
    pub block_extensions: Mutex<LruCache<Byte32, Option<packed::Bytes>>>,
}

impl Default for StoreCache {
    fn default() -> Self {
        StoreCache::from_config(StoreConfig::default())
    }
}

impl StoreCache {
    /// TODO(doc): @quake
    pub fn from_config(config: StoreConfig) -> Self {
        StoreCache {
            headers: Mutex::new(LruCache::new(config.header_cache_size)),
            cell_data: Mutex::new(LruCache::new(config.cell_data_cache_size)),
            cell_data_hash: Mutex::new(LruCache::new(config.cell_data_cache_size)),
            block_proposals: Mutex::new(LruCache::new(config.block_proposals_cache_size)),
            block_tx_hashes: Mutex::new(LruCache::new(config.block_tx_hashes_cache_size)),
            block_uncles: Mutex::new(LruCache::new(config.block_uncles_cache_size)),
            block_extensions: Mutex::new(LruCache::new(config.block_extensions_cache_size)),
        }
    }
}
