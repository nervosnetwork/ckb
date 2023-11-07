mod fetch;

use crate::store::SQLXPool;

use ckb_async_runtime::Handle;
use ckb_indexer_sync::{Error, Pool};
use ckb_jsonrpc_types::IndexerTip;

use std::sync::{Arc, RwLock};

/// Async handle to the indexer-r.
pub struct AsyncIndexerRHandle {
    store: SQLXPool,
    _pool: Option<Arc<RwLock<Pool>>>,
}

impl AsyncIndexerRHandle {
    /// Construct new AsyncIndexerRHandle instance
    pub fn new(_store: SQLXPool, _pool: Option<Arc<RwLock<Pool>>>) -> Self {
        Self {
            store: _store,
            _pool,
        }
    }
}

/// Handle to the indexer-r.
///
/// The handle is internally reference-counted and can be freely cloned.
/// A handle can be obtained using the IndexerRService::handle method.
pub struct IndexerRHandle {
    async_handle: AsyncIndexerRHandle,
    async_runtime: Handle,
}

impl IndexerRHandle {
    /// Construct new IndexerRHandle instance
    pub fn new(store: SQLXPool, pool: Option<Arc<RwLock<Pool>>>, async_handle: Handle) -> Self {
        Self {
            async_handle: AsyncIndexerRHandle::new(store, pool),
            async_runtime: async_handle,
        }
    }

    /// Get indexer current tip
    pub fn get_indexer_tip(&self) -> Result<Option<IndexerTip>, Error> {
        let future = self.async_handle.get_indexer_tip();
        self.async_runtime.block_on(future)
    }
}
