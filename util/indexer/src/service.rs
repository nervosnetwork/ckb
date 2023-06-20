//！The indexer service.

use crate::indexer::{self, extract_raw_data, CustomFilters, Indexer, Key, KeyPrefix, Value};
use crate::pool::Pool;
use crate::store::{IteratorDirection, RocksdbStore, SecondaryDB, Store};

use crate::error::Error;
use ckb_app_config::{DBConfig, IndexerConfig};
use ckb_async_runtime::{
    tokio::{self, time},
    Handle,
};
use ckb_db_schema::{COLUMN_BLOCK_BODY, COLUMN_BLOCK_HEADER, COLUMN_INDEX, COLUMN_META};
use ckb_jsonrpc_types::{
    IndexerCell, IndexerCellType, IndexerCellsCapacity, IndexerOrder, IndexerPagination,
    IndexerScriptSearchMode, IndexerScriptType, IndexerSearchKey, IndexerTip, IndexerTx,
    IndexerTxWithCell, IndexerTxWithCells, JsonBytes, Uint32,
};
use ckb_logger::{error, info};
use ckb_notify::NotifyController;
use ckb_stop_handler::{new_tokio_exit_rx, CancellationToken};
use ckb_store::ChainStore;
use ckb_types::{core, packed, prelude::*, H256};
use rocksdb::{prelude::*, Direction, IteratorMode};
use std::convert::TryInto;
use std::num::NonZeroUsize;
use std::sync::{Arc, RwLock};
use std::time::Duration;

const SUBSCRIBER_NAME: &str = "Indexer";
const DEFAULT_LOG_KEEP_NUM: usize = 1;
const DEFAULT_MAX_BACKGROUND_JOBS: usize = 6;

/// Indexer service
#[derive(Clone)]
pub struct IndexerService {
    store: RocksdbStore,
    secondary_db: SecondaryDB,
    pool: Option<Arc<RwLock<Pool>>>,
    poll_interval: Duration,
    async_handle: Handle,
    block_filter: Option<String>,
    cell_filter: Option<String>,
}

impl IndexerService {
    /// Construct new Indexer service instance from DBConfig and IndexerConfig
    pub fn new(ckb_db_config: &DBConfig, config: &IndexerConfig, async_handle: Handle) -> Self {
        let store_opts = Self::indexer_store_options(config);
        let store = RocksdbStore::new(&store_opts, &config.store);
        let pool = if config.index_tx_pool {
            Some(Arc::new(RwLock::new(Pool::default())))
        } else {
            None
        };

        let cf_names = vec![
            COLUMN_INDEX,
            COLUMN_META,
            COLUMN_BLOCK_HEADER,
            COLUMN_BLOCK_BODY,
        ];
        let secondary_opts = Self::indexer_secondary_options(config);
        let secondary_db = SecondaryDB::open_cf(
            &secondary_opts,
            &ckb_db_config.path,
            cf_names,
            config.secondary_path.to_string_lossy().to_string(),
        );

        Self {
            store,
            secondary_db,
            pool,
            async_handle,
            poll_interval: Duration::from_secs(config.poll_interval),
            block_filter: config.block_filter.clone(),
            cell_filter: config.cell_filter.clone(),
        }
    }

    /// Returns a handle to the indexer.
    ///
    /// The returned handle can be used to get data from indexer,
    /// and can be cloned to allow moving the Handle to other threads.
    pub fn handle(&self) -> IndexerHandle {
        IndexerHandle {
            store: self.store.clone(),
            pool: self.pool.clone(),
        }
    }

    /// Processes that handle index pool transaction and expect to be spawned to run in tokio runtime
    pub fn index_tx_pool(&self, notify_controller: NotifyController) {
        let service = self.clone();
        let stop: CancellationToken = new_tokio_exit_rx();

        self.async_handle.spawn(async move {
            let mut new_transaction_receiver = notify_controller
                .subscribe_new_transaction(SUBSCRIBER_NAME.to_string())
                .await;
            let mut reject_transaction_receiver = notify_controller
                .subscribe_reject_transaction(SUBSCRIBER_NAME.to_string())
                .await;

            loop {
                tokio::select! {
                    Some(tx_entry) = new_transaction_receiver.recv() => {
                        if let Some(pool) = service.pool.as_ref() {
                            pool.write().expect("acquire lock").new_transaction(&tx_entry.transaction);
                        }
                    }
                    Some((tx_entry, _reject)) = reject_transaction_receiver.recv() => {
                        if let Some(pool) = service.pool.as_ref() {
                            pool.write()
                            .expect("acquire lock")
                            .transaction_rejected(&tx_entry.transaction);
                        }
                    }
                    _ = stop.cancelled() => {
                        info!("Indexer received exit signal, exit now");
                        break
                    },
                    else => break,
                }
            }
        });
    }

    fn try_loop_sync(&self) {
        // assume that long fork will not happen >= 100 blocks.
        let keep_num = 100;
        if let Err(e) = self.secondary_db.try_catch_up_with_primary() {
            error!("secondary_db try_catch_up_with_primary error {}", e);
        }
        let indexer = Indexer::new(
            self.store.clone(),
            keep_num,
            1000,
            self.pool.clone(),
            CustomFilters::new(self.block_filter.as_deref(), self.cell_filter.as_deref()),
        );
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
    pub fn spawn_poll(&self, notify_controller: NotifyController) {
        let initial_service = self.clone();
        let initial_syncing = self
            .async_handle
            .spawn_blocking(move || initial_service.try_loop_sync());
        let stop: CancellationToken = new_tokio_exit_rx();
        let async_handle = self.async_handle.clone();
        let poll_service = self.clone();
        self.async_handle.spawn(async move {
            let _initial_finished = initial_syncing.await;
            let mut new_block_watcher = notify_controller
                .watch_new_block(SUBSCRIBER_NAME.to_string())
                .await;
            let mut interval = time::interval(poll_service.poll_interval);
            interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    Ok(_) = new_block_watcher.changed() => {
                        let service = poll_service.clone();
                        if let Err(e) = async_handle.spawn_blocking(move || {
                            service.try_loop_sync()
                        }).await {
                            error!("ckb indexer syncing join error {:?}", e);
                        }
                        new_block_watcher.borrow_and_update();
                    },
                    _ = interval.tick() => {
                        let service = poll_service.clone();
                        if let Err(e) = async_handle.spawn_blocking(move || {
                            service.try_loop_sync()
                        }).await {
                            error!("ckb indexer syncing join error {:?}", e);
                        }
                    }
                    _ = stop.cancelled() => {
                        info!("Indexer received exit signal, exit now");
                        break
                    },
                }
            }
        });
    }

    fn get_block_by_number(&self, block_number: u64) -> Option<core::BlockView> {
        let block_hash = self.secondary_db.get_block_hash(block_number)?;
        self.secondary_db.get_block(&block_hash)
    }

    fn indexer_store_options(config: &IndexerConfig) -> Options {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.set_keep_log_file_num(
            config
                .db_keep_log_file_num
                .map(NonZeroUsize::get)
                .unwrap_or(DEFAULT_LOG_KEEP_NUM),
        );
        opts.set_max_background_jobs(
            config
                .db_background_jobs
                .map(NonZeroUsize::get)
                .unwrap_or(DEFAULT_MAX_BACKGROUND_JOBS) as i32,
        );
        opts
    }

    fn indexer_secondary_options(config: &IndexerConfig) -> Options {
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
}

/// Handle to the indexer.
///
/// The handle is internally reference-counted and can be freely cloned.
/// A handle can be obtained using the IndexerService::handle method.
pub struct IndexerHandle {
    pub(crate) store: RocksdbStore,
    pub(crate) pool: Option<Arc<RwLock<Pool>>>,
}

impl IndexerHandle {
    /// Get indexer current tip
    pub fn get_indexer_tip(&self) -> Result<Option<IndexerTip>, Error> {
        let mut iter = self
            .store
            .iter([KeyPrefix::Header as u8 + 1], IteratorDirection::Reverse)
            .expect("iter Header should be OK");
        Ok(iter.next().map(|(key, _)| IndexerTip {
            block_hash: packed::Byte32::from_slice(&key[9..41])
                .expect("stored block key")
                .unpack(),
            block_number: core::BlockNumber::from_be_bytes(
                key[1..9].try_into().expect("stored block key"),
            )
            .into(),
        }))
    }

    /// Get cells by specified params
    pub fn get_cells(
        &self,
        search_key: IndexerSearchKey,
        order: IndexerOrder,
        limit: Uint32,
        after_cursor: Option<JsonBytes>,
    ) -> Result<IndexerPagination<IndexerCell>, Error> {
        let (prefix, from_key, direction, skip) = build_query_options(
            &search_key,
            KeyPrefix::CellLockScript,
            KeyPrefix::CellTypeScript,
            order,
            after_cursor,
        )?;
        let limit = limit.value() as usize;
        if limit == 0 {
            return Err(Error::invalid_params("limit should be greater than 0"));
        }

        let filter_script_type = match search_key.script_type {
            IndexerScriptType::Lock => IndexerScriptType::Type,
            IndexerScriptType::Type => IndexerScriptType::Lock,
        };
        let script_search_exact = matches!(
            search_key.script_search_mode,
            Some(IndexerScriptSearchMode::Exact)
        );
        let filter_options: FilterOptions = search_key.try_into()?;
        let mode = IteratorMode::From(from_key.as_ref(), direction);
        let snapshot = self.store.inner().snapshot();
        let iter = snapshot.iterator(mode).skip(skip);

        let mut last_key = Vec::new();
        let pool = self
            .pool
            .as_ref()
            .map(|pool| pool.read().expect("acquire lock"));
        let cells = iter
            .take_while(|(key, _value)| key.starts_with(&prefix))
            .filter_map(|(key, value)| {
                if script_search_exact {
                    // Exact match mode, check key length is equal to full script len + BlockNumber (8) + TxIndex (4) + OutputIndex (4)
                    if key.len() != prefix.len() + 16 {
                        return None;
                    }
                }
                let tx_hash = packed::Byte32::from_slice(&value).expect("stored tx hash");
                let index =
                    u32::from_be_bytes(key[key.len() - 4..].try_into().expect("stored index"));
                let out_point = packed::OutPoint::new(tx_hash, index);
                if pool
                    .as_ref()
                    .map(|pool| pool.is_consumed_by_pool_tx(&out_point))
                    .unwrap_or_default()
                {
                    return None;
                }
                let (block_number, tx_index, output, output_data) = Value::parse_cell_value(
                    &snapshot
                        .get(Key::OutPoint(&out_point).into_vec())
                        .expect("get OutPoint should be OK")
                        .expect("stored OutPoint"),
                );

                if let Some(prefix) = filter_options.script_prefix.as_ref() {
                    match filter_script_type {
                        IndexerScriptType::Lock => {
                            if !extract_raw_data(&output.lock())
                                .as_slice()
                                .starts_with(prefix)
                            {
                                return None;
                            }
                        }
                        IndexerScriptType::Type => {
                            if output.type_().is_none()
                                || !extract_raw_data(&output.type_().to_opt().unwrap())
                                    .as_slice()
                                    .starts_with(prefix)
                            {
                                return None;
                            }
                        }
                    }
                }

                if let Some([r0, r1]) = filter_options.script_len_range {
                    match filter_script_type {
                        IndexerScriptType::Lock => {
                            let script_len = extract_raw_data(&output.lock()).len();
                            if script_len < r0 || script_len > r1 {
                                return None;
                            }
                        }
                        IndexerScriptType::Type => {
                            let script_len = output
                                .type_()
                                .to_opt()
                                .map(|script| extract_raw_data(&script).len())
                                .unwrap_or_default();
                            if script_len < r0 || script_len > r1 {
                                return None;
                            }
                        }
                    }
                }

                if let Some([r0, r1]) = filter_options.output_data_len_range {
                    if output_data.len() < r0 || output_data.len() >= r1 {
                        return None;
                    }
                }

                if let Some([r0, r1]) = filter_options.output_capacity_range {
                    let capacity: core::Capacity = output.capacity().unpack();
                    if capacity < r0 || capacity >= r1 {
                        return None;
                    }
                }

                if let Some([r0, r1]) = filter_options.block_range {
                    if block_number < r0 || block_number >= r1 {
                        return None;
                    }
                }

                last_key = key.to_vec();

                Some(IndexerCell {
                    output: output.into(),
                    output_data: if filter_options.with_data {
                        Some(output_data.into())
                    } else {
                        None
                    },
                    out_point: out_point.into(),
                    block_number: block_number.into(),
                    tx_index: tx_index.into(),
                })
            })
            .take(limit)
            .collect::<Vec<_>>();

        Ok(IndexerPagination::new(cells, JsonBytes::from_vec(last_key)))
    }

    /// Get transaction by specified params
    pub fn get_transactions(
        &self,
        search_key: IndexerSearchKey,
        order: IndexerOrder,
        limit: Uint32,
        after_cursor: Option<JsonBytes>,
    ) -> Result<IndexerPagination<IndexerTx>, Error> {
        let (prefix, from_key, direction, skip) = build_query_options(
            &search_key,
            KeyPrefix::TxLockScript,
            KeyPrefix::TxTypeScript,
            order,
            after_cursor,
        )?;
        let limit = limit.value() as usize;
        if limit == 0 {
            return Err(Error::invalid_params("limit should be greater than 0"));
        }

        let (filter_script, filter_block_range) = if let Some(filter) = search_key.filter.as_ref() {
            if filter.script_len_range.is_some() {
                return Err(Error::invalid_params(
                    "doesn't support search_key.filter.script_len_range parameter",
                ));
            }
            if filter.output_data_len_range.is_some() {
                return Err(Error::invalid_params(
                    "doesn't support search_key.filter.output_data_len_range parameter",
                ));
            }
            if filter.output_capacity_range.is_some() {
                return Err(Error::invalid_params(
                    "doesn't support search_key.filter.output_capacity_range parameter",
                ));
            }
            let filter_script: Option<packed::Script> =
                filter.script.as_ref().map(|script| script.clone().into());
            let filter_block_range: Option<[core::BlockNumber; 2]> = filter
                .block_range
                .as_ref()
                .map(|r| [r.start().into(), r.end().into()]);
            (filter_script, filter_block_range)
        } else {
            (None, None)
        };

        let filter_script_type = match search_key.script_type {
            IndexerScriptType::Lock => IndexerScriptType::Type,
            IndexerScriptType::Type => IndexerScriptType::Lock,
        };
        let script_search_exact = matches!(
            search_key.script_search_mode,
            Some(IndexerScriptSearchMode::Exact)
        );

        let mode = IteratorMode::From(from_key.as_ref(), direction);
        let snapshot = self.store.inner().snapshot();
        let iter = snapshot.iterator(mode).skip(skip);

        if search_key.group_by_transaction.unwrap_or_default() {
            let mut tx_with_cells: Vec<IndexerTxWithCells> = Vec::new();
            let mut last_key = Vec::new();
            for (key, value) in iter.take_while(|(key, _value)| key.starts_with(&prefix)) {
                if script_search_exact {
                    // Exact match mode, check key length is equal to full script len + BlockNumber (8) + TxIndex (4) + CellIndex (4) + CellType (1)
                    if key.len() != prefix.len() + 17 {
                        continue;
                    }
                }
                let tx_hash: H256 = packed::Byte32::from_slice(&value)
                    .expect("stored tx hash")
                    .unpack();
                if tx_with_cells.len() == limit
                    && tx_with_cells.last_mut().unwrap().tx_hash != tx_hash
                {
                    break;
                }
                last_key = key.to_vec();
                let block_number = u64::from_be_bytes(
                    key[key.len() - 17..key.len() - 9]
                        .try_into()
                        .expect("stored block_number"),
                );
                let tx_index = u32::from_be_bytes(
                    key[key.len() - 9..key.len() - 5]
                        .try_into()
                        .expect("stored tx_index"),
                );
                let io_index = u32::from_be_bytes(
                    key[key.len() - 5..key.len() - 1]
                        .try_into()
                        .expect("stored io_index"),
                );
                let io_type = if *key.last().expect("stored io_type") == 0 {
                    IndexerCellType::Input
                } else {
                    IndexerCellType::Output
                };

                if let Some(filter_script) = filter_script.as_ref() {
                    let filter_script_matched = match filter_script_type {
                        IndexerScriptType::Lock => snapshot
                            .get(
                                Key::TxLockScript(
                                    filter_script,
                                    block_number,
                                    tx_index,
                                    io_index,
                                    match io_type {
                                        IndexerCellType::Input => indexer::CellType::Input,
                                        IndexerCellType::Output => indexer::CellType::Output,
                                    },
                                )
                                .into_vec(),
                            )
                            .expect("get TxLockScript should be OK")
                            .is_some(),
                        IndexerScriptType::Type => snapshot
                            .get(
                                Key::TxTypeScript(
                                    filter_script,
                                    block_number,
                                    tx_index,
                                    io_index,
                                    match io_type {
                                        IndexerCellType::Input => indexer::CellType::Input,
                                        IndexerCellType::Output => indexer::CellType::Output,
                                    },
                                )
                                .into_vec(),
                            )
                            .expect("get TxTypeScript should be OK")
                            .is_some(),
                    };
                    if !filter_script_matched {
                        continue;
                    }
                }

                if let Some([r0, r1]) = filter_block_range {
                    if block_number < r0 || block_number >= r1 {
                        continue;
                    }
                }

                let last_tx_hash_is_same = tx_with_cells
                    .last_mut()
                    .map(|last| {
                        if last.tx_hash == tx_hash {
                            last.cells.push((io_type.clone(), io_index.into()));
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or_default();

                if !last_tx_hash_is_same {
                    tx_with_cells.push(IndexerTxWithCells {
                        tx_hash,
                        block_number: block_number.into(),
                        tx_index: tx_index.into(),
                        cells: vec![(io_type, io_index.into())],
                    });
                }
            }

            Ok(IndexerPagination::new(
                tx_with_cells.into_iter().map(IndexerTx::Grouped).collect(),
                JsonBytes::from_vec(last_key),
            ))
        } else {
            let mut last_key = Vec::new();
            let txs = iter
                .take_while(|(key, _value)| key.starts_with(&prefix))
                .filter_map(|(key, value)| {
                    if script_search_exact {
                        // Exact match mode, check key length is equal to full script len + BlockNumber (8) + TxIndex (4) + CellIndex (4) + CellType (1)
                        if key.len() != prefix.len() + 17 {
                            return None;
                        }
                    }
                    let tx_hash = packed::Byte32::from_slice(&value).expect("stored tx hash");
                    let block_number = u64::from_be_bytes(
                        key[key.len() - 17..key.len() - 9]
                            .try_into()
                            .expect("stored block_number"),
                    );
                    let tx_index = u32::from_be_bytes(
                        key[key.len() - 9..key.len() - 5]
                            .try_into()
                            .expect("stored tx_index"),
                    );
                    let io_index = u32::from_be_bytes(
                        key[key.len() - 5..key.len() - 1]
                            .try_into()
                            .expect("stored io_index"),
                    );
                    let io_type = if *key.last().expect("stored io_type") == 0 {
                        IndexerCellType::Input
                    } else {
                        IndexerCellType::Output
                    };

                    if let Some(filter_script) = filter_script.as_ref() {
                        match filter_script_type {
                            IndexerScriptType::Lock => {
                                snapshot
                                    .get(
                                        Key::TxLockScript(
                                            filter_script,
                                            block_number,
                                            tx_index,
                                            io_index,
                                            match io_type {
                                                IndexerCellType::Input => indexer::CellType::Input,
                                                IndexerCellType::Output => {
                                                    indexer::CellType::Output
                                                }
                                            },
                                        )
                                        .into_vec(),
                                    )
                                    .expect("get TxLockScript should be OK")?;
                            }
                            IndexerScriptType::Type => {
                                snapshot
                                    .get(
                                        Key::TxTypeScript(
                                            filter_script,
                                            block_number,
                                            tx_index,
                                            io_index,
                                            match io_type {
                                                IndexerCellType::Input => indexer::CellType::Input,
                                                IndexerCellType::Output => {
                                                    indexer::CellType::Output
                                                }
                                            },
                                        )
                                        .into_vec(),
                                    )
                                    .expect("get TxTypeScript should be OK")?;
                            }
                        }
                    }

                    if let Some([r0, r1]) = filter_block_range {
                        if block_number < r0 || block_number >= r1 {
                            return None;
                        }
                    }

                    last_key = key.to_vec();
                    Some(IndexerTx::Ungrouped(IndexerTxWithCell {
                        tx_hash: tx_hash.unpack(),
                        block_number: block_number.into(),
                        tx_index: tx_index.into(),
                        io_index: io_index.into(),
                        io_type,
                    }))
                })
                .take(limit)
                .collect::<Vec<_>>();

            Ok(IndexerPagination::new(txs, JsonBytes::from_vec(last_key)))
        }
    }

    /// Get cells_capacity by specified search_key
    pub fn get_cells_capacity(
        &self,
        search_key: IndexerSearchKey,
    ) -> Result<Option<IndexerCellsCapacity>, Error> {
        let (prefix, from_key, direction, skip) = build_query_options(
            &search_key,
            KeyPrefix::CellLockScript,
            KeyPrefix::CellTypeScript,
            IndexerOrder::Asc,
            None,
        )?;
        let filter_script_type = match search_key.script_type {
            IndexerScriptType::Lock => IndexerScriptType::Type,
            IndexerScriptType::Type => IndexerScriptType::Lock,
        };
        let script_search_exact = matches!(
            search_key.script_search_mode,
            Some(IndexerScriptSearchMode::Exact)
        );
        let filter_options: FilterOptions = search_key.try_into()?;
        let mode = IteratorMode::From(from_key.as_ref(), direction);
        let snapshot = self.store.inner().snapshot();
        let iter = snapshot.iterator(mode).skip(skip);
        let pool = self
            .pool
            .as_ref()
            .map(|pool| pool.read().expect("acquire lock"));

        let capacity: u64 = iter
            .take_while(|(key, _value)| key.starts_with(&prefix))
            .filter_map(|(key, value)| {
                if script_search_exact {
                    // Exact match mode, check key length is equal to full script len + BlockNumber (8) + TxIndex (4) + OutputIndex (4)
                    if key.len() != prefix.len() + 16 {
                        return None;
                    }
                }
                let tx_hash = packed::Byte32::from_slice(value.as_ref()).expect("stored tx hash");
                let index =
                    u32::from_be_bytes(key[key.len() - 4..].try_into().expect("stored index"));
                let out_point = packed::OutPoint::new(tx_hash, index);
                if pool
                    .as_ref()
                    .map(|pool| pool.is_consumed_by_pool_tx(&out_point))
                    .unwrap_or_default()
                {
                    return None;
                }
                let (block_number, _tx_index, output, output_data) = Value::parse_cell_value(
                    &snapshot
                        .get(Key::OutPoint(&out_point).into_vec())
                        .expect("get OutPoint should be OK")
                        .expect("stored OutPoint"),
                );

                if let Some(prefix) = filter_options.script_prefix.as_ref() {
                    match filter_script_type {
                        IndexerScriptType::Lock => {
                            if !extract_raw_data(&output.lock())
                                .as_slice()
                                .starts_with(prefix)
                            {
                                return None;
                            }
                        }
                        IndexerScriptType::Type => {
                            if output.type_().is_none()
                                || !extract_raw_data(&output.type_().to_opt().unwrap())
                                    .as_slice()
                                    .starts_with(prefix)
                            {
                                return None;
                            }
                        }
                    }
                }

                if let Some([r0, r1]) = filter_options.script_len_range {
                    match filter_script_type {
                        IndexerScriptType::Lock => {
                            let script_len = extract_raw_data(&output.lock()).len();
                            if script_len < r0 || script_len > r1 {
                                return None;
                            }
                        }
                        IndexerScriptType::Type => {
                            let script_len = output
                                .type_()
                                .to_opt()
                                .map(|script| extract_raw_data(&script).len())
                                .unwrap_or_default();
                            if script_len < r0 || script_len > r1 {
                                return None;
                            }
                        }
                    }
                }

                if let Some([r0, r1]) = filter_options.output_data_len_range {
                    if output_data.len() < r0 || output_data.len() >= r1 {
                        return None;
                    }
                }

                if let Some([r0, r1]) = filter_options.output_capacity_range {
                    let capacity: core::Capacity = output.capacity().unpack();
                    if capacity < r0 || capacity >= r1 {
                        return None;
                    }
                }

                if let Some([r0, r1]) = filter_options.block_range {
                    if block_number < r0 || block_number >= r1 {
                        return None;
                    }
                }

                Some(Unpack::<core::Capacity>::unpack(&output.capacity()).as_u64())
            })
            .sum();

        let tip_mode = IteratorMode::From(&[KeyPrefix::Header as u8 + 1], Direction::Reverse);
        let mut tip_iter = snapshot.iterator(tip_mode);
        Ok(tip_iter.next().map(|(key, _value)| IndexerCellsCapacity {
            capacity: capacity.into(),
            block_hash: packed::Byte32::from_slice(&key[9..41])
                .expect("stored block key")
                .unpack(),
            block_number: core::BlockNumber::from_be_bytes(
                key[1..9].try_into().expect("stored block key"),
            )
            .into(),
        }))
    }
}

const MAX_PREFIX_SEARCH_SIZE: usize = u16::max_value() as usize;

// a helper fn to build query options from search paramters, returns prefix, from_key, direction and skip offset
fn build_query_options(
    search_key: &IndexerSearchKey,
    lock_prefix: KeyPrefix,
    type_prefix: KeyPrefix,
    order: IndexerOrder,
    after_cursor: Option<JsonBytes>,
) -> Result<(Vec<u8>, Vec<u8>, Direction, usize), Error> {
    let mut prefix = match search_key.script_type {
        IndexerScriptType::Lock => vec![lock_prefix as u8],
        IndexerScriptType::Type => vec![type_prefix as u8],
    };
    let script: packed::Script = search_key.script.clone().into();
    let args_len = script.args().len();
    if args_len > MAX_PREFIX_SEARCH_SIZE {
        return Err(Error::invalid_params(format!(
            "search_key.script.args len should be less than {MAX_PREFIX_SEARCH_SIZE}"
        )));
    }
    prefix.extend_from_slice(extract_raw_data(&script).as_slice());

    let (from_key, direction, skip) = match order {
        IndexerOrder::Asc => after_cursor.map_or_else(
            || (prefix.clone(), Direction::Forward, 0),
            |json_bytes| (json_bytes.as_bytes().into(), Direction::Forward, 1),
        ),
        IndexerOrder::Desc => after_cursor.map_or_else(
            || {
                (
                    [
                        prefix.clone(),
                        vec![0xff; MAX_PREFIX_SEARCH_SIZE - args_len],
                    ]
                    .concat(),
                    Direction::Reverse,
                    0,
                )
            },
            |json_bytes| (json_bytes.as_bytes().into(), Direction::Reverse, 1),
        ),
    };

    Ok((prefix, from_key, direction, skip))
}

struct FilterOptions {
    script_prefix: Option<Vec<u8>>,
    script_len_range: Option<[usize; 2]>,
    output_data_len_range: Option<[usize; 2]>,
    output_capacity_range: Option<[core::Capacity; 2]>,
    block_range: Option<[core::BlockNumber; 2]>,
    with_data: bool,
}

impl TryInto<FilterOptions> for IndexerSearchKey {
    type Error = Error;

    fn try_into(self) -> Result<FilterOptions, Error> {
        let IndexerSearchKey {
            filter, with_data, ..
        } = self;
        let filter = filter.unwrap_or_default();
        let script_prefix = if let Some(script) = filter.script {
            let script: packed::Script = script.into();
            if script.args().len() > MAX_PREFIX_SEARCH_SIZE {
                return Err(Error::invalid_params(format!(
                    "search_key.filter.script.args len should be less than {MAX_PREFIX_SEARCH_SIZE}"
                )));
            }
            let mut prefix = Vec::new();
            prefix.extend_from_slice(extract_raw_data(&script).as_slice());
            Some(prefix)
        } else {
            None
        };

        let script_len_range = filter.script_len_range.map(|range| {
            [
                Into::<u64>::into(range.start()) as usize,
                Into::<u64>::into(range.end()) as usize,
            ]
        });

        let output_data_len_range = filter.output_data_len_range.map(|range| {
            [
                Into::<u64>::into(range.start()) as usize,
                Into::<u64>::into(range.end()) as usize,
            ]
        });
        let output_capacity_range = filter.output_capacity_range.map(|range| {
            [
                core::Capacity::shannons(range.start().into()),
                core::Capacity::shannons(range.end().into()),
            ]
        });
        let block_range = filter
            .block_range
            .map(|r| [r.start().into(), r.end().into()]);

        Ok(FilterOptions {
            script_prefix,
            script_len_range,
            output_data_len_range,
            output_capacity_range,
            block_range,
            with_data: with_data.unwrap_or(true),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::RocksdbStore;
    use ckb_jsonrpc_types::{IndexerRange, IndexerSearchKeyFilter};
    use ckb_types::{
        bytes::Bytes,
        core::{
            capacity_bytes, BlockBuilder, Capacity, EpochNumberWithFraction, HeaderBuilder,
            ScriptHashType, TransactionBuilder,
        },
        packed::{CellInput, CellOutputBuilder, OutPoint, Script, ScriptBuilder},
        H256,
    };

    fn new_store(prefix: &str) -> RocksdbStore {
        let tmp_dir = tempfile::Builder::new().prefix(prefix).tempdir().unwrap();
        RocksdbStore::new(
            &RocksdbStore::default_options(),
            tmp_dir.path().to_str().unwrap(),
        )
        // Indexer::new(store, 10, 1)
    }

    #[test]
    fn rpc() {
        let store = new_store("rpc");
        let pool = Arc::new(RwLock::new(Pool::default()));
        let indexer = Indexer::new(store.clone(), 10, 100, None, CustomFilters::new(None, None));
        let rpc = IndexerHandle {
            store,
            pool: Some(Arc::clone(&pool)),
        };

        // setup test data
        let lock_script1 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"lock_script1".to_vec()).pack())
            .build();

        let lock_script2 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Type.into())
            .args(Bytes::from(b"lock_script2".to_vec()).pack())
            .build();

        let type_script1 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"type_script1".to_vec()).pack())
            .build();

        let type_script2 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Type.into())
            .args(Bytes::from(b"type_script2".to_vec()).pack())
            .build();

        let cellbase0 = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(0))
            .witness(Script::default().into_witness())
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx00 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .type_(Some(type_script1.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx01 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(2000).pack())
                    .lock(lock_script2.clone())
                    .type_(Some(type_script2.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let block0 = BlockBuilder::default()
            .transaction(cellbase0)
            .transaction(tx00.clone())
            .transaction(tx01.clone())
            .header(HeaderBuilder::default().number(0.pack()).build())
            .build();

        indexer.append(&block0).unwrap();

        let (mut pre_tx0, mut pre_tx1, mut pre_block) = (tx00, tx01, block0);
        let total_blocks = 255;
        for i in 1..total_blocks {
            let cellbase = TransactionBuilder::default()
                .input(CellInput::new_cellbase_input(i + 1))
                .witness(Script::default().into_witness())
                .output(
                    CellOutputBuilder::default()
                        .capacity(capacity_bytes!(1000).pack())
                        .lock(lock_script1.clone())
                        .build(),
                )
                .output_data(Bytes::from(i.to_string()).pack())
                .build();

            pre_tx0 = TransactionBuilder::default()
                .input(CellInput::new(OutPoint::new(pre_tx0.hash(), 0), 0))
                .output(
                    CellOutputBuilder::default()
                        .capacity(capacity_bytes!(1000).pack())
                        .lock(lock_script1.clone())
                        .type_(Some(type_script1.clone()).pack())
                        .build(),
                )
                .output_data(Default::default())
                .build();

            pre_tx1 = TransactionBuilder::default()
                .input(CellInput::new(OutPoint::new(pre_tx1.hash(), 0), 0))
                .output(
                    CellOutputBuilder::default()
                        .capacity(capacity_bytes!(2000).pack())
                        .lock(lock_script2.clone())
                        .type_(Some(type_script2.clone()).pack())
                        .build(),
                )
                .output_data(Default::default())
                .build();

            pre_block = BlockBuilder::default()
                .transaction(cellbase)
                .transaction(pre_tx0.clone())
                .transaction(pre_tx1.clone())
                .header(
                    HeaderBuilder::default()
                        .number((pre_block.number() + 1).pack())
                        .parent_hash(pre_block.hash())
                        .epoch(
                            EpochNumberWithFraction::new(
                                pre_block.number() + 1,
                                pre_block.number(),
                                1000,
                            )
                            .pack(),
                        )
                        .build(),
                )
                .build();

            indexer.append(&pre_block).unwrap();
        }

        // test get_tip rpc
        let tip = rpc.get_indexer_tip().unwrap().unwrap();
        assert_eq!(Unpack::<H256>::unpack(&pre_block.hash()), tip.block_hash);
        assert_eq!(pre_block.number(), tip.block_number.value());

        // test get_cells rpc
        let cells_page_1 = rpc
            .get_cells(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                150.into(),
                None,
            )
            .unwrap();
        let cells_page_2 = rpc
            .get_cells(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    with_data: Some(false),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                150.into(),
                Some(cells_page_1.last_cursor),
            )
            .unwrap();

        assert_eq!(
            total_blocks as usize + 1,
            cells_page_1.objects.len() + cells_page_2.objects.len(),
            "total size should be cellbase cells count + 1 (last block live cell)"
        );

        let output_data: packed::Bytes =
            cells_page_1.objects[10].output_data.clone().unwrap().into();
        assert_eq!(
            output_data.raw_data().to_vec(),
            b"10",
            "block #10 cellbase output_data should be 10"
        );

        assert!(
            cells_page_2.objects[10].output_data.is_none(),
            "cellbase output_data should be none when the params with_data is false"
        );

        let desc_cells_page_1 = rpc
            .get_cells(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    ..Default::default()
                },
                IndexerOrder::Desc,
                150.into(),
                None,
            )
            .unwrap();

        let desc_cells_page_2 = rpc
            .get_cells(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    ..Default::default()
                },
                IndexerOrder::Desc,
                150.into(),
                Some(desc_cells_page_1.last_cursor),
            )
            .unwrap();

        assert_eq!(
            total_blocks as usize + 1,
            desc_cells_page_1.objects.len() + desc_cells_page_2.objects.len(),
            "total size should be cellbase cells count + 1 (last block live cell)"
        );
        assert_eq!(
            desc_cells_page_1.objects.first().unwrap().out_point,
            cells_page_2.objects.last().unwrap().out_point
        );

        let filter_cells_page_1 = rpc
            .get_cells(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    filter: Some(IndexerSearchKeyFilter {
                        block_range: Some(IndexerRange::new(100, 200)),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                60.into(),
                None,
            )
            .unwrap();

        let filter_cells_page_2 = rpc
            .get_cells(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    filter: Some(IndexerSearchKeyFilter {
                        block_range: Some(IndexerRange::new(100, 200)),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                60.into(),
                Some(filter_cells_page_1.last_cursor),
            )
            .unwrap();

        assert_eq!(
            100,
            filter_cells_page_1.objects.len() + filter_cells_page_2.objects.len(),
            "total size should be filtered cellbase cells (100~199)"
        );

        let filter_empty_type_script_cells_page_1 = rpc
            .get_cells(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    filter: Some(IndexerSearchKeyFilter {
                        script_len_range: Some(IndexerRange::new(0, 1)),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                150.into(),
                None,
            )
            .unwrap();

        let filter_empty_type_script_cells_page_2 = rpc
            .get_cells(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    filter: Some(IndexerSearchKeyFilter {
                        script_len_range: Some(IndexerRange::new(0, 1)),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                150.into(),
                Some(filter_empty_type_script_cells_page_1.last_cursor),
            )
            .unwrap();

        assert_eq!(
            total_blocks as usize,
            filter_empty_type_script_cells_page_1.objects.len()
                + filter_empty_type_script_cells_page_2.objects.len(),
            "total size should be cellbase cells count (empty type script)"
        );

        // test get_transactions rpc
        let txs_page_1 = rpc
            .get_transactions(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                500.into(),
                None,
            )
            .unwrap();
        let txs_page_2 = rpc
            .get_transactions(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                500.into(),
                Some(txs_page_1.last_cursor),
            )
            .unwrap();

        assert_eq!(total_blocks as usize * 3 - 1, txs_page_1.objects.len() + txs_page_2.objects.len(), "total size should be cellbase tx count + total_block * 2 - 1 (genesis block only has one tx)");

        let desc_txs_page_1 = rpc
            .get_transactions(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    ..Default::default()
                },
                IndexerOrder::Desc,
                500.into(),
                None,
            )
            .unwrap();
        let desc_txs_page_2 = rpc
            .get_transactions(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    ..Default::default()
                },
                IndexerOrder::Desc,
                500.into(),
                Some(desc_txs_page_1.last_cursor),
            )
            .unwrap();

        assert_eq!(total_blocks as usize * 3 - 1, desc_txs_page_1.objects.len() + desc_txs_page_2.objects.len(), "total size should be cellbase tx count + total_block * 2 - 1 (genesis block only has one tx)");
        assert_eq!(
            desc_txs_page_1.objects.first().unwrap().tx_hash(),
            txs_page_2.objects.last().unwrap().tx_hash()
        );

        let filter_txs_page_1 = rpc
            .get_transactions(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    filter: Some(IndexerSearchKeyFilter {
                        block_range: Some(IndexerRange::new(100, 200)),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                200.into(),
                None,
            )
            .unwrap();

        let filter_txs_page_2 = rpc
            .get_transactions(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    filter: Some(IndexerSearchKeyFilter {
                        block_range: Some(IndexerRange::new(100, 200)),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                200.into(),
                Some(filter_txs_page_1.last_cursor),
            )
            .unwrap();

        assert_eq!(
            300,
            filter_txs_page_1.objects.len() + filter_txs_page_2.objects.len(),
            "total size should be filtered blocks count * 3 (100~199 * 3)"
        );

        // test get_transactions rpc group by tx hash
        let txs_page_1 = rpc
            .get_transactions(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    group_by_transaction: Some(true),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                500.into(),
                None,
            )
            .unwrap();
        let txs_page_2 = rpc
            .get_transactions(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    group_by_transaction: Some(true),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                500.into(),
                Some(txs_page_1.last_cursor),
            )
            .unwrap();

        assert_eq!(
            total_blocks as usize * 2,
            txs_page_1.objects.len() + txs_page_2.objects.len(),
            "total size should be cellbase tx count + total_block"
        );

        let desc_txs_page_1 = rpc
            .get_transactions(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    group_by_transaction: Some(true),
                    ..Default::default()
                },
                IndexerOrder::Desc,
                500.into(),
                None,
            )
            .unwrap();
        let desc_txs_page_2 = rpc
            .get_transactions(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    group_by_transaction: Some(true),
                    ..Default::default()
                },
                IndexerOrder::Desc,
                500.into(),
                Some(desc_txs_page_1.last_cursor),
            )
            .unwrap();

        assert_eq!(
            total_blocks as usize * 2,
            desc_txs_page_1.objects.len() + desc_txs_page_2.objects.len(),
            "total size should be cellbase tx count + total_block"
        );
        assert_eq!(
            desc_txs_page_1.objects.first().unwrap().tx_hash(),
            txs_page_2.objects.last().unwrap().tx_hash()
        );

        let filter_txs_page_1 = rpc
            .get_transactions(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    group_by_transaction: Some(true),
                    filter: Some(IndexerSearchKeyFilter {
                        block_range: Some(IndexerRange::new(100, 200)),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                150.into(),
                None,
            )
            .unwrap();

        let filter_txs_page_2 = rpc
            .get_transactions(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    group_by_transaction: Some(true),
                    filter: Some(IndexerSearchKeyFilter {
                        block_range: Some(IndexerRange::new(100, 200)),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                150.into(),
                Some(filter_txs_page_1.last_cursor),
            )
            .unwrap();

        assert_eq!(
            200,
            filter_txs_page_1.objects.len() + filter_txs_page_2.objects.len(),
            "total size should be filtered blocks count * 2 (100~199 * 2)"
        );

        // test get_cells_capacity rpc
        let capacity = rpc
            .get_cells_capacity(IndexerSearchKey {
                script: lock_script1.clone().into(),
                ..Default::default()
            })
            .unwrap()
            .unwrap();

        assert_eq!(
            1000 * 100000000 * (total_blocks + 1),
            capacity.capacity.value(),
            "cellbases + last block live cell"
        );

        let capacity = rpc
            .get_cells_capacity(IndexerSearchKey {
                script: lock_script2.into(),
                ..Default::default()
            })
            .unwrap()
            .unwrap();

        assert_eq!(
            2000 * 100000000,
            capacity.capacity.value(),
            "last block live cell"
        );

        // test get_cells rpc with tx-pool overlay
        let pool_tx = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(pre_tx0.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .type_(Some(type_script1).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();
        pool.write().unwrap().new_transaction(&pool_tx);

        let cells_page_1 = rpc
            .get_cells(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                150.into(),
                None,
            )
            .unwrap();
        let cells_page_2 = rpc
            .get_cells(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                150.into(),
                Some(cells_page_1.last_cursor),
            )
            .unwrap();

        assert_eq!(
            total_blocks as usize,
            cells_page_1.objects.len() + cells_page_2.objects.len(),
            "total size should be cellbase cells count (last block live cell was consumed by a pending tx in the pool)"
        );

        // test get_cells_capacity rpc with tx-pool overlay
        let capacity = rpc
            .get_cells_capacity(IndexerSearchKey {
                script: lock_script1.into(),
                ..Default::default()
            })
            .unwrap()
            .unwrap();

        assert_eq!(
            1000 * 100000000 * total_blocks,
            capacity.capacity.value(),
            "cellbases (last block live cell was consumed by a pending tx in the pool)"
        );
    }

    #[test]
    fn script_search_mode_rpc() {
        let store = new_store("script_search_mode_rpc");
        let indexer = Indexer::new(store.clone(), 10, 100, None, CustomFilters::new(None, None));
        let rpc = IndexerHandle { store, pool: None };

        // setup test data
        let lock_script1 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Type.into())
            .args(Bytes::from(b"lock_script1".to_vec()).pack())
            .build();

        let lock_script11 = ScriptBuilder::default()
            .code_hash(lock_script1.code_hash())
            .hash_type(ScriptHashType::Type.into())
            .args(Bytes::from(b"lock_script11".to_vec()).pack())
            .build();

        let type_script1 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"type_script1".to_vec()).pack())
            .build();

        let type_script11 = ScriptBuilder::default()
            .code_hash(type_script1.code_hash())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"type_script11".to_vec()).pack())
            .build();

        let cellbase0 = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(0))
            .witness(Script::default().into_witness())
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx00 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .type_(Some(type_script1.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx01 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(2000).pack())
                    .lock(lock_script11.clone())
                    .type_(Some(type_script11.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let block0 = BlockBuilder::default()
            .transaction(cellbase0)
            .transaction(tx00.clone())
            .transaction(tx01.clone())
            .header(HeaderBuilder::default().number(0.pack()).build())
            .build();

        indexer.append(&block0).unwrap();

        let (mut pre_tx0, mut pre_tx1, mut pre_block) = (tx00, tx01, block0);
        let total_blocks = 255;
        for i in 1..total_blocks {
            let cellbase = TransactionBuilder::default()
                .input(CellInput::new_cellbase_input(i + 1))
                .witness(Script::default().into_witness())
                .output(
                    CellOutputBuilder::default()
                        .capacity(capacity_bytes!(1000).pack())
                        .lock(lock_script1.clone())
                        .build(),
                )
                .output_data(Bytes::from(i.to_string()).pack())
                .build();

            pre_tx0 = TransactionBuilder::default()
                .input(CellInput::new(OutPoint::new(pre_tx0.hash(), 0), 0))
                .output(
                    CellOutputBuilder::default()
                        .capacity(capacity_bytes!(1000).pack())
                        .lock(lock_script1.clone())
                        .type_(Some(type_script1.clone()).pack())
                        .build(),
                )
                .output_data(Default::default())
                .build();

            pre_tx1 = TransactionBuilder::default()
                .input(CellInput::new(OutPoint::new(pre_tx1.hash(), 0), 0))
                .output(
                    CellOutputBuilder::default()
                        .capacity(capacity_bytes!(2000).pack())
                        .lock(lock_script11.clone())
                        .type_(Some(type_script11.clone()).pack())
                        .build(),
                )
                .output_data(Default::default())
                .build();

            pre_block = BlockBuilder::default()
                .transaction(cellbase)
                .transaction(pre_tx0.clone())
                .transaction(pre_tx1.clone())
                .header(
                    HeaderBuilder::default()
                        .number((pre_block.number() + 1).pack())
                        .parent_hash(pre_block.hash())
                        .epoch(
                            EpochNumberWithFraction::new(
                                pre_block.number() + 1,
                                pre_block.number(),
                                1000,
                            )
                            .pack(),
                        )
                        .build(),
                )
                .build();

            indexer.append(&pre_block).unwrap();
        }

        // test get_cells rpc with prefix search mode
        let cells = rpc
            .get_cells(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                1000.into(),
                None,
            )
            .unwrap();

        assert_eq!(
            total_blocks as usize + 2,
            cells.objects.len(),
            "total size should be cellbase cells count + 2 (last block live cell: lock_script1 and lock_script11)"
        );

        // test get_cells rpc with exact search mode
        let cells = rpc
            .get_cells(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    script_search_mode: Some(IndexerScriptSearchMode::Exact),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                1000.into(),
                None,
            )
            .unwrap();

        assert_eq!(
            total_blocks as usize + 1,
            cells.objects.len(),
            "total size should be cellbase cells count + 1 (last block live cell: lock_script1)"
        );

        // test get_transactions rpc with exact search mode
        let txs = rpc
            .get_transactions(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    script_search_mode: Some(IndexerScriptSearchMode::Exact),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                1000.into(),
                None,
            )
            .unwrap();

        assert_eq!(total_blocks as usize * 3 - 1, txs.objects.len(), "total size should be cellbase tx count + total_block * 2 - 1 (genesis block only has one tx)");

        // test get_transactions rpc group by tx hash with exact search mode
        let txs = rpc
            .get_transactions(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    script_search_mode: Some(IndexerScriptSearchMode::Exact),
                    group_by_transaction: Some(true),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                1000.into(),
                None,
            )
            .unwrap();

        assert_eq!(
            total_blocks as usize * 2,
            txs.objects.len(),
            "total size should be cellbase tx count + total_block"
        );

        // test get_cells_capacity rpc with exact search mode
        let capacity = rpc
            .get_cells_capacity(IndexerSearchKey {
                script: lock_script1.clone().into(),
                script_search_mode: Some(IndexerScriptSearchMode::Exact),
                ..Default::default()
            })
            .unwrap()
            .unwrap();

        assert_eq!(
            1000 * 100000000 * (total_blocks + 1),
            capacity.capacity.value(),
            "cellbases + last block live cell"
        );

        // test get_cells_capacity rpc with prefix search mode (by default)
        let capacity = rpc
            .get_cells_capacity(IndexerSearchKey {
                script: lock_script1.into(),
                ..Default::default()
            })
            .unwrap()
            .unwrap();

        assert_eq!(
            1000 * 100000000 * (total_blocks + 1) + 2000 * 100000000,
            capacity.capacity.value()
        );
    }
}
