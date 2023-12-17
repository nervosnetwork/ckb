mod async_indexer_handle;

pub use async_indexer_handle::*;

use crate::store::SQLXPool;

use ckb_async_runtime::Handle;
use ckb_indexer_sync::{Error, Pool};
use ckb_jsonrpc_types::IndexerTip;

use std::sync::{Arc, RwLock};

/// Handle to the rich-indexer.
///
/// The handle is internally reference-counted and can be freely cloned.
/// A handle can be obtained using the RichIndexerService::handle method.
#[derive(Clone)]
pub struct RichIndexerHandle {
    async_handle: AsyncRichIndexerHandle,
    async_runtime: Handle,
}

impl RichIndexerHandle {
    /// Construct new RichIndexerHandle instance
    pub fn new(store: SQLXPool, pool: Option<Arc<RwLock<Pool>>>, async_handle: Handle) -> Self {
        Self {
            async_handle: AsyncRichIndexerHandle::new(store, pool),
            async_runtime: async_handle,
        }
    }

    /// Get indexer current tip
    pub fn get_indexer_tip(&self) -> Result<Option<IndexerTip>, Error> {
        let future = self.async_handle.query_indexer_tip();
        self.async_runtime.block_on(future)
    }
}
