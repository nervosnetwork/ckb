//! The built-in synchronization service in CKB can provide block synchronization services for indexers.

pub(crate) mod custom_filters;
pub(crate) mod error;
pub(crate) mod pool;
mod source;
pub(crate) mod store;

pub use crate::custom_filters::CustomFilters;
pub use crate::error::Error;
pub use crate::pool::{Pool, PoolService};
pub use crate::source::IndexerSource;
pub use crate::store::SecondaryDB;

use ckb_app_config::{DBConfig, IndexerSyncConfig};
use ckb_async_runtime::{
    Handle,
    tokio::{self, time},
};
use ckb_logger::{error, info};
use ckb_notify::NotifyController;
use ckb_stop_handler::{CancellationToken, has_received_stop_signal, new_tokio_exit_rx};
use ckb_types::{
    H256,
    core::{BlockNumber, BlockView},
    packed::Byte32,
};
use rocksdb::{LogLevel, prelude::*};

use std::marker::Send;
use std::sync::{Arc, RwLock};
use std::thread::sleep;
use std::time::Duration;

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
    let secondary_opts = indexer_secondary_options();
    SecondaryDB::open_cf(
        &secondary_opts,
        &ckb_db_config.path,
        store::INDEXER_COLUMNS,
        config.secondary_path.to_string_lossy().to_string(),
    )
}

/// Indexer sync service
#[derive(Clone)]
pub struct IndexerSyncService {
    secondary_db: IndexerSource,
    pool_service: PoolService,
    poll_interval: Duration,
    async_handle: Handle,
    init_tip_hash: Option<H256>,
}

impl IndexerSyncService {
    /// Construct new Indexer sync service instance
    pub fn new(
        secondary_db: impl Into<IndexerSource>,
        pool_service: PoolService,
        config: &IndexerSyncConfig,
        async_handle: Handle,
        init_tip_hash: Option<H256>,
    ) -> Self {
        Self {
            secondary_db: secondary_db.into(),
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
        let Some(hash) = &self.init_tip_hash else {
            return;
        };
        while !has_received_stop_signal() {
            match self
                .secondary_db
                .refresh()
                .and_then(|_| self.secondary_db.header(&hash.into()))
            {
                Ok(Some(header)) => {
                    // A lazy Freezer view has no header before its first refresh.
                    // Recheck the persisted tip here so restart cannot rewind it.
                    match indexer_service.tip() {
                        Ok(tip) => {
                            if tip.is_none_or(|(n, _)| n < header.number()) {
                                indexer_service.set_init_tip(header.number(), hash);
                            }
                            return;
                        }
                        Err(e) => error!("Cannot read persisted indexer tip: {e}"),
                    }
                }
                Ok(None) => (),
                Err(e) => error!("Cannot read initial indexer tip: {e}"),
            }
            sleep(Duration::from_secs(1));
        }
    }

    fn try_loop_sync<I: IndexerSync>(&self, indexer: I) {
        if let Err(e) = self.sync_available(&indexer) {
            error!("{}: {e}; retrying on the next poll", indexer.get_identity());
        }
    }

    fn sync_available<I: IndexerSync>(&self, indexer: &I) -> Result<(), Error> {
        self.secondary_db.refresh()?;
        while !has_received_stop_signal() {
            let tip = indexer.tip()?;
            let number = tip.as_ref().map_or(0, |(n, _)| n + 1);
            let block = self.secondary_db.block(number)?;
            if let Some((height, hash)) = tip {
                let parent = match &block {
                    Some(child) => Some(child.parent_hash()),
                    None => self.secondary_db.canonical_hash(height)?,
                };
                let orphaned = match parent {
                    Some(canonical) => canonical != hash,
                    None => self
                        .secondary_db
                        .tip()?
                        .is_some_and(|tip| tip.number() < height),
                };
                if orphaned {
                    info!("{} rollback {height}, {hash}", indexer.get_identity());
                    indexer.rollback()?;
                    self.secondary_db.refresh()?;
                    continue;
                }
            }
            let Some(block) = block else { break };
            info!(
                "{} append {number}, {}",
                indexer.get_identity(),
                block.hash()
            );
            indexer.append(&block)?;
        }
        Ok(())
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

                match secondary_db.refresh().and_then(|_| secondary_db.tip()) {
                    Ok(Some(header))
                        if indexer_service.tip().is_ok_and(|tip| {
                            tip.is_some_and(|(n, _)| {
                                n.abs_diff(header.number()) <= INDEXER_NODE_TIP_GAP
                            })
                        }) =>
                    {
                        break;
                    }
                    Err(e) => error!("Cannot read node tip for indexer: {e}"),
                    _ => (),
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });

        self.pool_service
            .index_tx_pool(notify_controller, check_index_tx_pool_ready);
    }
}

fn indexer_secondary_options() -> Options {
    let mut opts = Options::default();
    opts.create_if_missing(true);
    opts.create_missing_column_families(true);
    opts.set_log_level(LogLevel::Warn);
    opts
}
