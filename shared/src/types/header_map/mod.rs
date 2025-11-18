use ckb_store::{ChainDB, ChainStore};
use ckb_types::packed::Byte32;
use ckb_types::{U256, core::HeaderView};
use std::sync::Arc;

use super::HeaderIndexView;

/// HeaderMap provides a read-only view of headers from the chain store
/// with on-demand computation of total_difficulty
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

    pub fn get(&self, hash: &Byte32) -> Option<HeaderIndexView> {
        let header = self.store.get_block_header(hash)?;
        let total_difficulty = self.compute_total_difficulty(hash, &header)?;
        Some((header, total_difficulty).into())
    }

    /// Insert is a no-op now - headers are written directly via ChainStore
    /// This method is kept for API compatibility
    pub fn insert(&self, _view: HeaderIndexView) -> Option<()> {
        Some(())
    }

    /// Remove is a no-op - headers in COLUMN_BLOCK_HEADER are managed by ChainStore
    pub fn remove(&self, _hash: &Byte32) {
        // Headers are managed by ChainStore, not removed here
    }

    fn compute_total_difficulty(&self, hash: &Byte32, header: &HeaderView) -> Option<U256> {
        // Fast path: check if BlockExt exists (block is verified)
        if let Some(block_ext) = self.store.get_block_ext(hash) {
            return Some(block_ext.total_difficulty);
        }

        // Genesis block
        if header.number() == 0 {
            return Some(header.difficulty());
        }

        // Recursive path: compute from parent
        let parent_hash = header.parent_hash();
        let parent_header = self.store.get_block_header(&parent_hash)?;
        let parent_total_difficulty =
            self.compute_total_difficulty(&parent_hash, &parent_header)?;
        Some(parent_total_difficulty + header.difficulty())
    }
}
