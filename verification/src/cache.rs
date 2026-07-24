//! TX verification cache

use ckb_script::TransactionState;
use ckb_types::{
    core::{Capacity, Cycle, EntryCompleted, TransactionView},
    packed::Byte32,
    prelude::Entity,
};
use std::sync::Arc;

/// Semantic key for [`TxVerificationCache`].
///
/// Script verification covers witnesses, while [`TransactionView::hash`]
/// deliberately does not.  Keeping the witness hash behind this type makes a
/// raw transaction hash impossible to pass to the cache accidentally.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub struct TxVerificationCacheKey([u8; 32]);

impl TxVerificationCacheKey {
    /// Build the only valid verification-cache key for `tx`.
    pub fn from_transaction(tx: &TransactionView) -> Self {
        let mut hash = [0u8; 32];
        hash.copy_from_slice(tx.witness_hash().as_slice());
        Self(hash)
    }

    /// Return the underlying witness hash for diagnostics.
    pub fn as_witness_hash(&self) -> Byte32 {
        Byte32::new(self.0)
    }
}

/// TX verification lru cache
pub type TxVerificationCache = lru::LruCache<TxVerificationCacheKey, CacheEntry>;

const CACHE_SIZE: usize = 1000 * 30;

/// Initialize cache
pub fn init_cache() -> TxVerificationCache {
    lru::LruCache::new(CACHE_SIZE)
}

/// TX verification lru entry
pub type CacheEntry = Completed;

/// Suspended state
#[derive(Clone, Debug)]
pub struct Suspended {
    /// Cached tx fee
    pub fee: Capacity,
    /// State
    pub state: Arc<TransactionState>,
}

/// Completed entry
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completed {
    /// Cached tx cycles
    pub cycles: Cycle,
    /// Cached tx fee
    pub fee: Capacity,
}

impl From<Completed> for EntryCompleted {
    fn from(value: Completed) -> Self {
        EntryCompleted {
            cycles: value.cycles,
            fee: value.fee,
        }
    }
}
