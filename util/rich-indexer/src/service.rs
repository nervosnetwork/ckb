//！The rich-indexer service.

use crate::indexer::RichIndexer;
use crate::store::SQLXPool;
use crate::{AsyncRichIndexerHandle, RichIndexerHandle};

use ckb_app_config::IndexerConfig;
use ckb_async_runtime::Handle;
use ckb_indexer_sync::{CustomFilters, IndexerSyncService, PoolService, SecondaryDB};
use ckb_notify::NotifyController;

pub(crate) const SUBSCRIBER_NAME: &str = "Rich-Indexer";

/// Rich-Indexer service
#[derive(Clone)]
pub struct RichIndexerService {
    store: SQLXPool,
    sync: IndexerSyncService,
    block_filter: Option<String>,
    cell_filter: Option<String>,
    async_handle: Handle,
}

impl RichIndexerService {
    /// Construct new RichIndexerService instance
    pub fn new(
        ckb_db: SecondaryDB,
        pool_service: PoolService,
        config: &IndexerConfig,
        async_handle: Handle,
    ) -> Self {
        let mut store = SQLXPool::default();
        async_handle
            .block_on(store.connect(&config.rich_indexer))
            .expect("Failed to connect to rich-indexer database");

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

    fn get_indexer(&self) -> RichIndexer {
        // assume that long fork will not happen >= 100 blocks.
        let keep_num = 100;
        RichIndexer::new(
            self.store.clone(),
            keep_num,
            1000,
            self.sync.pool(),
            CustomFilters::new(self.block_filter.as_deref(), self.cell_filter.as_deref()),
            self.async_handle.clone(),
        )
    }

    /// Spawn a poller to sync data from ckb node.
    pub fn spawn_poll(&self, notify_controller: NotifyController) {
        self.sync.spawn_poll(
            notify_controller,
            SUBSCRIBER_NAME.to_string(),
            self.get_indexer(),
        )
    }

    /// Returns a handle to the rich-indexer.
    ///
    /// The returned handle can be used to get data from rich-indexer,
    /// and can be cloned to allow moving the Handle to other threads.
    pub fn handle(&self) -> RichIndexerHandle {
        RichIndexerHandle::new(
            self.store.clone(),
            self.sync.pool(),
            self.async_handle.clone(),
        )
    }

    /// Returns a handle to the rich-indexer.
    ///
    /// The returned handle can be used to get data from rich-indexer,
    /// and can be cloned to allow moving the Handle to other threads.
    pub fn async_handle(&self) -> AsyncRichIndexerHandle {
        AsyncRichIndexerHandle::new(self.store.clone(), self.sync.pool())
    }
}
