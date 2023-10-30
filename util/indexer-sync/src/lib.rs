//! The built-in synchronization service in CKB can provide block synchronization services for indexers.

pub(crate) mod error;
pub(crate) mod pool;
pub(crate) mod store;

pub use crate::error::Error;
pub use crate::pool::{Pool, PoolService};
pub use crate::store::SecondaryDB;

use ckb_app_config::{DBConfig, IndexerSyncConfig};
use ckb_async_runtime::{
    tokio::{self, time},
    Handle,
};
use ckb_db_schema::{COLUMN_BLOCK_BODY, COLUMN_BLOCK_HEADER, COLUMN_INDEX, COLUMN_META};
use ckb_logger::{debug, error, info};
use ckb_notify::NotifyController;
use ckb_stop_handler::{new_tokio_exit_rx, CancellationToken};
use ckb_store::ChainStore;
use ckb_types::{
    core::{self, BlockNumber, BlockView},
    packed::Byte32,
};
use rocksdb::prelude::*;

use std::marker::Send;
use std::num::NonZeroUsize;
use std::sync::{Arc, RwLock};
use std::time::Duration;

const DEFAULT_LOG_KEEP_NUM: usize = 1;

/// Trait for an indexer's synchronization interface
pub trait IndexerSync {
    /// Retrieves the tip of the indexer
    fn tip(&self) -> Result<Option<(BlockNumber, Byte32)>, Error>;
    /// Appends a new block to the indexer
    fn append(&self, block: &BlockView) -> Result<(), Error>;
    /// Rollback the indexer to a previous state
    fn rollback(&self) -> Result<(), Error>;
}

/// Construct new secondary db instance from DBConfig
pub fn new_secondary_db(ckb_db_config: &DBConfig, config: &IndexerSyncConfig) -> SecondaryDB {
    let cf_names = vec![
        COLUMN_INDEX,
        COLUMN_META,
        COLUMN_BLOCK_HEADER,
        COLUMN_BLOCK_BODY,
    ];
    let secondary_opts = indexer_secondary_options(config);
    SecondaryDB::open_cf(
        &secondary_opts,
        &ckb_db_config.path,
        cf_names,
        config.secondary_path.to_string_lossy().to_string(),
    )
}

/// Indexer sync service
#[derive(Clone)]
pub struct IndexerSyncService {
    secondary_db: SecondaryDB,
    pool_service: PoolService,
    poll_interval: Duration,
    async_handle: Handle,
}

impl IndexerSyncService {
    /// Construct new Indexer service instance from DBConfig and IndexerConfig
    pub fn new(
        secondary_db: SecondaryDB,
        pool_service: PoolService,
        config: &IndexerSyncConfig,
        async_handle: Handle,
    ) -> Self {
        Self {
            secondary_db,
            pool_service,
            poll_interval: Duration::from_secs(config.poll_interval),
            async_handle,
        }
    }

    fn try_loop_sync<I: IndexerSync>(&self, indexer: I)
    where
        I: IndexerSync + Clone + Send + 'static,
    {
        if let Err(e) = self.secondary_db.try_catch_up_with_primary() {
            error!("secondary_db try_catch_up_with_primary error {}", e);
        }
        loop {
            if let Some((tip_number, tip_hash)) = indexer.tip().expect("get tip should be OK") {
                match self.get_block_by_number(tip_number + 1) {
                    Some(block) => {
                        if block.parent_hash() == tip_hash {
                            info!("append {}, {}", block.number(), block.hash());
                            indexer.append(&block).expect("append block should be OK");
                        } else {
                            info!("rollback {}, {}", tip_number, tip_hash);
                            indexer.rollback().expect("rollback block should be OK");
                        }
                    }
                    None => {
                        break;
                    }
                }
            } else {
                match self.get_block_by_number(0) {
                    Some(block) => indexer.append(&block).expect("append block should be OK"),
                    None => {
                        error!("ckb node returns an empty genesis block");
                        break;
                    }
                }
            }
        }
    }

    /// Processes that handle block cell and expect to be spawned to run in tokio runtime
    pub fn spawn_poll<I>(
        &self,
        notify_controller: NotifyController,
        subscriber_name: String,
        indexer_service: I,
    ) where
        I: IndexerSync + Clone + Send + 'static,
    {
        let initial_service = self.clone();
        let indexer = indexer_service.clone();
        let initial_syncing = self
            .async_handle
            .spawn_blocking(move || initial_service.try_loop_sync(indexer));
        let stop: CancellationToken = new_tokio_exit_rx();
        let async_handle = self.async_handle.clone();
        let poll_service = self.clone();
        self.async_handle.spawn(async move {
            let _initial_finished = initial_syncing.await;
            let mut new_block_watcher = notify_controller.watch_new_block(subscriber_name).await;
            let mut interval = time::interval(poll_service.poll_interval);
            interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
            loop {
                let indexer = indexer_service.clone();
                tokio::select! {
                    Ok(_) = new_block_watcher.changed() => {
                        let service = poll_service.clone();
                        if let Err(e) = async_handle.spawn_blocking(move || {
                            service.try_loop_sync(indexer)
                        }).await {
                            error!("ckb indexer syncing join error {:?}", e);
                        }
                        new_block_watcher.borrow_and_update();
                    },
                    _ = interval.tick() => {
                        let service = poll_service.clone();
                        if let Err(e) = async_handle.spawn_blocking(move || {
                            service.try_loop_sync(indexer)
                        }).await {
                            error!("ckb indexer syncing join error {:?}", e);
                        }
                    }
                    _ = stop.cancelled() => {
                        debug!("Indexer received exit signal, exit now");
                        break
                    },
                }
            }
        });
    }

    pub fn pool(&self) -> Option<Arc<RwLock<Pool>>> {
        self.pool_service.pool()
    }

    fn get_block_by_number(&self, block_number: u64) -> Option<core::BlockView> {
        let block_hash = self.secondary_db.get_block_hash(block_number)?;
        self.secondary_db.get_block(&block_hash)
    }
}

fn indexer_secondary_options(config: &IndexerSyncConfig) -> Options {
    let mut opts = Options::default();
    opts.create_if_missing(true);
    opts.create_missing_column_families(true);
    opts.set_keep_log_file_num(
        config
            .db_keep_log_file_num
            .map(NonZeroUsize::get)
            .unwrap_or(DEFAULT_LOG_KEEP_NUM),
    );
    opts
}
