//！The indexer service.

use ckb_indexer_sync::Error;
use ckb_jsonrpc_types::IndexerTip;
use ckb_notify::NotifyController;

pub struct IndexerRService {}

impl IndexerRService {
    pub fn new() -> Self {
        Self {}
    }

    pub fn spawn_poll(&self, _notify_controller: NotifyController) {
        unimplemented!("spawn_poll")
    }

    /// Returns a handle to the indexer-r.
    ///
    /// The returned handle can be used to get data from indexer-r,
    /// and can be cloned to allow moving the Handle to other threads.
    pub fn handle(&self) -> IndexerRHandle {
        IndexerRHandle {}
    }
}

/// Handle to the indexer-r.
///
/// The handle is internally reference-counted and can be freely cloned.
/// A handle can be obtained using the IndexerRService::handle method.
pub struct IndexerRHandle {}

impl IndexerRHandle {
    /// Get indexer current tip
    pub fn get_indexer_tip(&self) -> Result<Option<IndexerTip>, Error> {
        unimplemented!("get_indexer_tip")
    }
}
