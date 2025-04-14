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
    Handle,
    tokio::{self, time},
};
use ckb_db_schema::{
    COLUMN_BLOCK_BODY, COLUMN_BLOCK_EXTENSION, COLUMN_BLOCK_HEADER, COLUMN_BLOCK_PROPOSAL_IDS,
    COLUMN_BLOCK_UNCLE, COLUMN_INDEX, COLUMN_META,
};
use ckb_logger::{error, info};
use ckb_notify::NotifyController;
use ckb_stop_handler::{CancellationToken, has_received_stop_signal, new_tokio_exit_rx};
use ckb_store::ChainStore;
use ckb_types::{
    H256,
    core::{self, BlockNumber, BlockView},
    packed::Byte32,
    prelude::*,
};
use rocksdb::prelude::*;

use std::marker::Send;
use std::num::NonZeroUsize;
use std::sync::{Arc, RwLock};
use std::thread::sleep;
use std::time::Duration;

const DEFAULT_LOG_KEEP_NUM: usize = 1;
const INDEXER_NODE_TIP_GAP: u64 = 10;

/// Trait for an indexer's synchronization interface
pub trait IndexerSync {
    /// Retrieves the tip of the indexer
    fn tip(&self) -> Result<Option<(BlockNumber, Byte32)>, Error>;
    /// Appends a new block to the indexer
    fn append(&self, block: &BlockView) -> Result<(), Error>;
    /// Rollback the indexer to a previous state
    fn rollback(&self) -> Result<(), Error>;
    /// Get indexer identity
    fn get_identity(&self) -> &str;
    /// Set init tip
    fn set_init_tip(&self, init_tip_number: u64, init_tip_hash: &H256);
}

/// Construct new secondary db instance
pub fn new_secondary_db(ckb_db_config: &DBConfig, config: &IndexerSyncConfig) -> SecondaryDB {
    let cf_names = vec![
        COLUMN_INDEX,
        COLUMN_META,
        COLUMN_BLOCK_HEADER,
        COLUMN_BLOCK_BODY,
        COLUMN_BLOCK_UNCLE,
        COLUMN_BLOCK_PROPOSAL_IDS,
        COLUMN_BLOCK_EXTENSION,
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
    init_tip_hash: Option<H256>,
}

impl IndexerSyncService {
    /// Construct new Indexer sync service instance
    pub fn new(
        secondary_db: SecondaryDB,
        pool_service: PoolService,
        config: &IndexerSyncConfig,
        async_handle: Handle,
        init_tip_hash: Option<H256>,
    ) -> Self {
        Self {
            secondary_db,
            pool_service,
            poll_interval: Duration::from_secs(config.poll_interval),
            async_handle,
            init_tip_hash,
        }
    }

    /// Apply init tip
    fn apply_init_tip<I>(&self, indexer_service: I)
    where
        I: IndexerSync + Clone + Send + 'static,
    {
        if let Some(init_tip_hash) = &self.init_tip_hash {
            let indexer_tip = indexer_service
                .tip()
                .expect("indexer_service tip should be OK");
            if let Some((indexer_tip, _)) = indexer_tip {
                if let Some(init_tip) = self.secondary_db.get_block_header(&init_tip_hash.pack()) {
                    if indexer_tip >= init_tip.number() {
                        return;
                    }
                }
            }
            loop {
                if has_received_stop_signal() {
                    info!("apply_init_tip received exit signal, exit now");
                    break;
                }

                if let Err(e) = self.secondary_db.try_catch_up_with_primary() {
                    error!("secondary_db try_catch_up_with_primary error {}", e);
                }
                if let Some(header) = self.secondary_db.get_block_header(&init_tip_hash.pack()) {
                    let init_tip_number = header.number();
                    indexer_service.set_init_tip(init_tip_number, init_tip_hash);
                    break;
                }
                sleep(Duration::from_secs(1));
            }
        }
    }

    fn try_loop_sync<I>(&self, indexer: I)
    where
        I: IndexerSync + Clone + Send + 'static,
    {
        if let Err(e) = self.secondary_db.try_catch_up_with_primary() {
            error!("secondary_db try_catch_up_with_primary error {}", e);
        }
        loop {
            if has_received_stop_signal() {
                info!("try_loop_sync received exit signal, exit now");
                break;
            }

            match indexer.tip() {
                Ok(Some((tip_number, tip_hash))) => {
                    match self.get_block_by_number(tip_number + 1) {
                        Some(block) => {
                            if block.parent_hash() == tip_hash {
                                info!(
                                    "{} append {}, {}",
                                    indexer.get_identity(),
                                    block.number(),
                                    block.hash()
                                );
                                if let Err(e) = indexer.append(&block) {
                                    error!("Failed to append block: {}. Will attempt to retry.", e);
                                }
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
                }
                Ok(None) => match self.get_block_by_number(0) {
                    Some(block) => {
                        if let Err(e) = indexer.append(&block) {
                            error!("Failed to append block: {}. Will attempt to retry.", e);
                        }
                    }
                    None => {
                        error!("CKB node returns an empty genesis block");
                        break;
                    }
                },
                Err(e) => {
                    error!("Failed to get tip: {}", e);
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
        // Initial sync
        let initial_service = self.clone();
        let indexer = indexer_service.clone();
        let initial_syncing = self.async_handle.spawn_blocking(move || {
            initial_service.apply_init_tip(indexer.clone());
            initial_service.try_loop_sync(indexer)
        });

        // Follow-up sync
        let stop: CancellationToken = new_tokio_exit_rx();
        let async_handle = self.async_handle.clone();
        let poll_service = self.clone();
        self.async_handle.spawn(async move {
            let _initial_finished = initial_syncing.await;
            if stop.is_cancelled() {
                info!("Indexer received exit signal, cancel new_block_watcher task, exit now");
                return;
            }

            info!("initial_syncing finished");

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
                        info!("{} received exit signal, exit now", indexer_service.get_identity());
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

    /// Index transaction pool
    pub fn index_tx_pool<I>(&mut self, indexer_service: I, notify_controller: NotifyController)
    where
        I: IndexerSync + Clone + Send + 'static,
    {
        let secondary_db = self.secondary_db.clone();
        let check_index_tx_pool_ready = self.async_handle.spawn_blocking(move || {
            loop {
                if has_received_stop_signal() {
                    info!("check_index_tx_pool_ready received exit signal, exit now");
                    break;
                }

                if let Err(e) = secondary_db.try_catch_up_with_primary() {
                    error!("secondary_db try_catch_up_with_primary error {}", e);
                }
                if let (Some(header), Ok(Some((indexer_tip, _)))) =
                    (secondary_db.get_tip_header(), indexer_service.tip())
                {
                    let node_tip = header.number();
                    if node_tip - indexer_tip <= INDEXER_NODE_TIP_GAP {
                        break;
                    }
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });

        self.pool_service
            .index_tx_pool(notify_controller, check_index_tx_pool_ready);
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
