//！The indexer-r service.

use crate::indexer::IndexerR;
use crate::store::SQLXPool;

use ckb_app_config::IndexerConfig;
use ckb_async_runtime::Handle;
use ckb_indexer_sync::{CustomFilters, Error, IndexerSyncService, PoolService, SecondaryDB};
use ckb_jsonrpc_types::IndexerTip;
use ckb_notify::NotifyController;

const SUBSCRIBER_NAME: &str = "Indexer-R";

/// Indexer-R service
#[derive(Clone)]
pub struct IndexerRService {
    store: SQLXPool,
    sync: IndexerSyncService,
    block_filter: Option<String>,
    cell_filter: Option<String>,
}

impl IndexerRService {
    pub fn new(
        ckb_db: SecondaryDB,
        pool_service: PoolService,
        config: &IndexerConfig,
        async_handle: Handle,
    ) -> Self {
        let store = SQLXPool::new(10, 0, 60, 1800, 30);
        let sync =
            IndexerSyncService::new(ckb_db, pool_service, &config.into(), async_handle.clone());
        Self {
            store,
            sync,
            block_filter: config.block_filter.clone(),
            cell_filter: config.cell_filter.clone(),
        }
    }

    fn get_indexer(&self) -> IndexerR {
        // assume that long fork will not happen >= 100 blocks.
        let keep_num = 100;
        IndexerR::new(
            self.store.clone(),
            keep_num,
            1000,
            self.sync.pool(),
            CustomFilters::new(self.block_filter.as_deref(), self.cell_filter.as_deref()),
        )
    }

    pub fn spawn_poll(&self, notify_controller: NotifyController) {
        self.sync.spawn_poll(
            notify_controller,
            SUBSCRIBER_NAME.to_string(),
            self.get_indexer(),
        )
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
