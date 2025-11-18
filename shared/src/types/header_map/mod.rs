use ckb_store::{ChainDB, ChainStore};
use ckb_types::packed::Byte32;
use ckb_types::core::HeaderView;
use std::sync::Arc;

/// HeaderMap provides a read-only view of headers from the chain store
pub struct HeaderMap {
    store: Arc<ChainDB>,
}

impl HeaderMap {
    pub fn new(store: Arc<ChainDB>) -> Self {
        Self { store }
    }

    pub fn contains_key(&self, hash: &Byte32) -> bool {
        self.store.get_block_header(hash).is_some()
    }

    pub fn get(&self, hash: &Byte32) -> Option<HeaderView> {
        self.store.get_block_header(hash)
    }

    /// Remove is a no-op - headers in COLUMN_BLOCK_HEADER are managed by ChainStore
    pub fn remove(&self, _hash: &Byte32) {
        // Headers are managed by ChainStore, not removed here
    }
}
