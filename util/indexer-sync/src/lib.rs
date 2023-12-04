//! The built-in synchronization service in CKB can provide block synchronization services for indexers.

pub(crate) mod custom_filters;
pub(crate) mod error;
pub(crate) mod pool;
pub(crate) mod store;

pub use crate::custom_filters::CustomFilters;
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
    /// Appends new blocks to the indexer
    fn append_bulk(&self, block: &[BlockView]) -> Result<(), Error>;
    /// Rollback the indexer to a previous state
    fn rollback(&self) -> Result<(), Error>;
    /// Get indexer identity
    fn get_identity(&self) -> &str;
}

/// Construct new secondary db instance
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
    /// Construct new Indexer sync service instance
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
                            info!(
                                "{} append {}, {}",
                                indexer.get_identity(),
                                block.number(),
                                block.hash()
                            );
                            indexer.append(&block).expect("append block should be OK");
                        } else {
                            info!(
                                "{} rollback {}, {}",
                                indexer.get_identity(),
                                tip_number,
                                tip_hash
                            );
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

    // Bulk insert blocks without the need to verify the parent of new block.
    fn try_loop_sync_fast<I: IndexerSync>(&self, indexer: I)
    where
        I: IndexerSync + Clone + Send + 'static,
    {
        const BULK_SIZE: u64 = 10;
        if let Err(e) = self.secondary_db.try_catch_up_with_primary() {
            error!("secondary_db try_catch_up_with_primary error {}", e);
        }
        let chain_tip = self.get_tip().expect("get chain tip should be OK");
        let indexer_tip = {
            if let Some((tip_number, _)) = indexer.tip().expect("get tip should be OK") {
                tip_number
            } else {
                let block = self
                    .get_block_by_number(0)
                    .expect("get genesis block should be OK");
                indexer.append(&block).expect("append block should be OK");
                0
            }
        };
        // assume that long fork will not happen >= 100 blocks.
        let target: u64 = chain_tip.0.saturating_sub(100);
        for start in (indexer_tip + 1..=target).step_by(BULK_SIZE as usize) {
            let end = (start + BULK_SIZE - 1).min(target);
            let blocks: Vec<BlockView> = (start..=end)
                .map(|number| {
                    self.get_block_by_number(number)
                        .expect("get block should be OK")
                })
                .collect();
            indexer
                .append_bulk(&blocks)
                .expect("append blocks should be OK");
            blocks.iter().for_each(|block| {
                info!(
                    "{} append {}, {}",
                    indexer.get_identity(),
                    block.number(),
                    block.hash()
                );
            });
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
        // Initial sync
        let initial_service = self.clone();
        let indexer = indexer_service.clone();
        let initial_syncing = self.async_handle.spawn_blocking(move || {
            initial_service.try_loop_sync_fast(indexer.clone());
            initial_service.try_loop_sync(indexer)
        });

        // Follow-up sync
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
                            error!("{} syncing join error {:?}", indexer_service.get_identity(), e);
                        }
                        new_block_watcher.borrow_and_update();
                    },
                    _ = interval.tick() => {
                        let service = poll_service.clone();
                        if let Err(e) = async_handle.spawn_blocking(move || {
                            service.try_loop_sync(indexer)
                        }).await {
                            error!("{} syncing join error {:?}", indexer_service.get_identity(), e);
                        }
                    }
                    _ = stop.cancelled() => {
                        debug!("{} received exit signal, exit now", indexer_service.get_identity());
                        break
                    },
                }
            }
        });
    }

    /// Get index data based on transaction pool synchronization
    pub fn pool(&self) -> Option<Arc<RwLock<Pool>>> {
        self.pool_service.pool()
    }

    fn get_block_by_number(&self, block_number: u64) -> Option<core::BlockView> {
        let block_hash = self.secondary_db.get_block_hash(block_number)?;
        self.secondary_db.get_block(&block_hash)
    }

    fn get_tip(&self) -> Option<(BlockNumber, Byte32)> {
        self.secondary_db
            .get_tip_header()
            .map(|h| (h.number(), h.hash()))
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
