//！The indexer-r service.

use crate::indexer::IndexerR;
use crate::store::SQLXPool;
use crate::{AsyncIndexerRHandle, IndexerRHandle};

use ckb_app_config::IndexerConfig;
use ckb_async_runtime::Handle;
use ckb_indexer_sync::{CustomFilters, IndexerSyncService, PoolService, SecondaryDB};
use ckb_notify::NotifyController;

const SUBSCRIBER_NAME: &str = "Indexer-R";

/// Indexer-R service
#[derive(Clone)]
pub struct IndexerRService {
    store: SQLXPool,
    sync: IndexerSyncService,
    block_filter: Option<String>,
    cell_filter: Option<String>,
    async_handle: Handle,
}

impl IndexerRService {
    pub fn new(
        ckb_db: SecondaryDB,
        pool_service: PoolService,
        config: &IndexerConfig,
        async_handle: Handle,
    ) -> Self {
        let mut store = SQLXPool::new(10, 0, 60, 1800, 30);
        async_handle
            .block_on(store.connect(
                &config.indexer_r.db_type,
                &config.indexer_r.db_name,
                &config.indexer_r.db_host,
                config.indexer_r.db_port,
                &config.indexer_r.db_user,
                &config.indexer_r.password,
            ))
            .expect("Failed to connect to indexer-r database");
        let sync =
            IndexerSyncService::new(ckb_db, pool_service, &config.into(), async_handle.clone());
        Self {
            store,
            sync,
            block_filter: config.block_filter.clone(),
            cell_filter: config.cell_filter.clone(),
            async_handle,
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
        IndexerRHandle::new(
            self.store.clone(),
            self.sync.pool(),
            self.async_handle.clone(),
        )
    }

    /// Returns a handle to the indexer-r.
    ///
    /// The returned handle can be used to get data from indexer-r,
    /// and can be cloned to allow moving the Handle to other threads.
    pub fn async_handle(&self) -> AsyncIndexerRHandle {
        AsyncIndexerRHandle::new(self.store.clone(), self.sync.pool())
    }
}
