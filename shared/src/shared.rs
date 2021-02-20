//! TODO(doc): @quake
use crate::{Snapshot, SnapshotMgr};
use arc_swap::Guard;
use ckb_app_config::{BlockAssemblerConfig, DBConfig, NotifyConfig, StoreConfig, TxPoolConfig};
use ckb_async_runtime::{new_global_runtime, Handle};
use ckb_chain_spec::consensus::Consensus;
use ckb_chain_spec::SpecError;
use ckb_constant::store::TX_INDEX_UPPER_BOUND;
use ckb_constant::sync::MAX_TIP_AGE;
use ckb_db::{Direction, IteratorMode, RocksDB};
use ckb_db_schema::COLUMN_BLOCK_BODY;
use ckb_db_schema::{COLUMNS, COLUMN_NUMBER_HASH};
use ckb_error::{Error, InternalErrorKind};
use ckb_freezer::Freezer;
use ckb_notify::{NotifyController, NotifyService, PoolTransactionEntry};
use ckb_proposal_table::{ProposalTable, ProposalView};
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_store::{ChainDB, ChainStore};
use ckb_tx_pool::{
    error::Reject, TokioRwLock, TxEntry, TxPool, TxPoolController, TxPoolServiceBuilder,
};
use ckb_types::{
    core::{service, BlockNumber, EpochExt, EpochNumber, HeaderView},
    packed::{self, Byte32},
    prelude::*,
    U256,
};
use ckb_verification::cache::TxVerifyCache;
use faketime::unix_time_as_millis;
use std::cmp;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const FREEZER_INTERVAL: Duration = Duration::from_secs(60);
const THRESHOLD_EPOCH: EpochNumber = 2;
const MAX_FREEZE_LIMIT: BlockNumber = 30_000;

/// An owned permission to close on a freezer thread
pub struct FreezerClose {
    stopped: Arc<AtomicBool>,
    stop: StopHandler<()>,
}

impl Drop for FreezerClose {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.stop.try_send();
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        if let Some(ref mut stop) = self.async_stop {
            stop.try_send();
        }
    }
}

/// TODO(doc): @quake
#[derive(Clone)]
pub struct Shared {
    pub(crate) store: ChainDB,
    pub(crate) tx_pool_controller: TxPoolController,
    pub(crate) notify_controller: NotifyController,
    pub(crate) txs_verify_cache: Arc<TokioRwLock<TxVerifyCache>>,
    pub(crate) consensus: Arc<Consensus>,
    pub(crate) snapshot_mgr: Arc<SnapshotMgr>,
    pub(crate) async_handle: Handle,
    // async stop handle, only test will be assigned
    pub(crate) async_stop: Option<StopHandler<()>>,
    pub(crate) ibd_finished: Arc<AtomicBool>,
}

impl Shared {
    /// TODO(doc): @quake
    pub fn init(
        store: ChainDB,
        consensus: Consensus,
        tx_pool_config: TxPoolConfig,
        notify_config: NotifyConfig,
        block_assembler_config: Option<BlockAssemblerConfig>,
        async_handle: Handle,
        async_stop: Option<StopHandler<()>>,
    ) -> Result<(Self, ProposalTable), Error> {
        let (tip_header, epoch) = Self::init_store(&store, &consensus)?;
        let total_difficulty = store
            .get_block_ext(&tip_header.hash())
            .ok_or_else(|| InternalErrorKind::Database.other("failed to get tip's block_ext"))?
            .total_difficulty;
        let (proposal_table, proposal_view) = Self::init_proposal_table(&store, &consensus);

        let consensus = Arc::new(consensus);

        let txs_verify_cache = Arc::new(TokioRwLock::new(TxVerifyCache::new(
            tx_pool_config.max_verify_cache_size,
        )));
        let snapshot = Arc::new(Snapshot::new(
            tip_header,
            total_difficulty,
            epoch,
            store.get_snapshot(),
            proposal_view,
            Arc::clone(&consensus),
        ));
        let snapshot_mgr = Arc::new(SnapshotMgr::new(Arc::clone(&snapshot)));
        let notify_controller = NotifyService::new(notify_config).start(Some("NotifyService"));

        let mut tx_pool_builder = TxPoolServiceBuilder::new(
            tx_pool_config,
            Arc::clone(&snapshot),
            block_assembler_config,
            Arc::clone(&txs_verify_cache),
            Arc::clone(&snapshot_mgr),
        );

        let notify_pending = notify_controller.clone();
        tx_pool_builder.register_pending(Box::new(move |tx_pool: &mut TxPool, entry: TxEntry| {
            // update statics
            tx_pool.update_statics_for_add_tx(entry.size, entry.cycles);

            // notify
            let notify_tx_entry = PoolTransactionEntry {
                transaction: entry.rtx.transaction,
                cycles: entry.cycles,
                size: entry.size,
                fee: entry.fee,
            };
            notify_pending.notify_new_transaction(notify_tx_entry);
        }));

        let notify_proposed = notify_controller.clone();
        tx_pool_builder.register_proposed(Box::new(
            move |tx_pool: &mut TxPool, entry: TxEntry, new: bool| {
                // update statics
                if new {
                    tx_pool.update_statics_for_add_tx(entry.size, entry.cycles);
                }

                // notify
                let notify_tx_entry = PoolTransactionEntry {
                    transaction: entry.rtx.transaction,
                    cycles: entry.cycles,
                    size: entry.size,
                    fee: entry.fee,
                };
                notify_proposed.notify_proposed_transaction(notify_tx_entry);
            },
        ));

        tx_pool_builder.register_committed(Box::new(
            move |tx_pool: &mut TxPool, entry: TxEntry| {
                tx_pool.update_statics_for_remove_tx(entry.size, entry.cycles);
            },
        ));

        let notify_reject = notify_controller.clone();
        tx_pool_builder.register_reject(Box::new(
            move |tx_pool: &mut TxPool, entry: TxEntry, reject: Reject| {
                // update statics
                tx_pool.update_statics_for_remove_tx(entry.size, entry.cycles);

                // notify
                let notify_tx_entry = PoolTransactionEntry {
                    transaction: entry.rtx.transaction,
                    cycles: entry.cycles,
                    size: entry.size,
                    fee: entry.fee,
                };
                notify_reject.notify_reject_transaction(notify_tx_entry, reject);
            },
        ));

        let tx_pool_controller = tx_pool_builder.start(&async_handle);

        let shared = Shared {
            store,
            consensus,
            txs_verify_cache,
            snapshot_mgr,
            tx_pool_controller,
            notify_controller,
            async_handle,
            async_stop,
            ibd_finished: Arc::new(AtomicBool::new(false)),
        };

        Ok((shared, proposal_table))
    }

    pub(crate) fn init_proposal_table(
        store: &ChainDB,
        consensus: &Consensus,
    ) -> (ProposalTable, ProposalView) {
        let proposal_window = consensus.tx_proposal_window();
        let tip_number = store.get_tip_header().expect("store inited").number();
        let mut proposal_ids = ProposalTable::new(proposal_window);
        let proposal_start = tip_number.saturating_sub(proposal_window.farthest());
        for bn in proposal_start..=tip_number {
            if let Some(hash) = store.get_block_hash(bn) {
                let mut ids_set = HashSet::new();
                if let Some(ids) = store.get_block_proposal_txs_ids(&hash) {
                    ids_set.extend(ids)
                }

                if let Some(us) = store.get_block_uncles(&hash) {
                    for u in us.data().into_iter() {
                        ids_set.extend(u.proposals().into_iter());
                    }
                }
                proposal_ids.insert(bn, ids_set);
            }
        }
        let dummy_proposals = ProposalView::default();
        let (_, proposals) = proposal_ids.finalize(&dummy_proposals, tip_number);
        (proposal_ids, proposals)
    }

    pub(crate) fn init_store(
        store: &ChainDB,
        consensus: &Consensus,
    ) -> Result<(HeaderView, EpochExt), Error> {
        match store
            .get_tip_header()
            .and_then(|header| store.get_current_epoch_ext().map(|epoch| (header, epoch)))
        {
            Some((tip_header, epoch)) => {
                if let Some(genesis_hash) = store.get_block_hash(0) {
                    let expect_genesis_hash = consensus.genesis_hash();
                    if genesis_hash == expect_genesis_hash {
                        Ok((tip_header, epoch))
                    } else {
                        Err(SpecError::GenesisMismatch {
                            expected: expect_genesis_hash,
                            actual: genesis_hash,
                        }
                        .into())
                    }
                } else {
                    Err(InternalErrorKind::Database
                        .other("genesis does not exist in database")
                        .into())
                }
            }
            None => store.init(&consensus).map(|_| {
                (
                    consensus.genesis_block().header(),
                    consensus.genesis_epoch_ext().to_owned(),
                )
            }),
        }
    }

    /// Spawn freeze background thread that periodically checks and moves ancient data from the kv database into the freezer.
    pub fn spawn_freeze(&self) -> Option<FreezerClose> {
        if let Some(freezer) = self.store.freezer() {
            ckb_logger::info!("Freezer enable");
            let (signal_sender, signal_receiver) =
                ckb_channel::bounded::<()>(service::SIGNAL_CHANNEL_SIZE);
            let shared = self.clone();
            let thread = thread::Builder::new()
                .spawn(move || loop {
                    match signal_receiver.recv_timeout(FREEZER_INTERVAL) {
                        Err(_) => {
                            if let Err(e) = shared.freeze() {
                                ckb_logger::error!("Freezer error {}", e);
                                break;
                            }
                        }
                        Ok(_) => {
                            ckb_logger::info!("Freezer closing");
                            break;
                        }
                    }
                })
                .expect("Start FreezerService failed");

            let stop = StopHandler::new(SignalSender::Crossbeam(signal_sender), Some(thread));
            return Some(FreezerClose {
                stopped: Arc::clone(&freezer.stopped),
                stop,
            });
        }
        None
    }

    fn freeze(&self) -> Result<(), Error> {
        let freezer = self.store.freezer().expect("freezer inited");
        let snapshot = self.snapshot();
        let current_epoch = snapshot.epoch_ext().number();

        if self.is_initial_block_download() {
            ckb_logger::trace!("is_initial_block_download freeze skip");
            return Ok(());
        }

        if current_epoch <= THRESHOLD_EPOCH {
            ckb_logger::trace!("freezer loaf");
            return Ok(());
        }

        let limit_block_hash = snapshot
            .get_epoch_index(current_epoch + 1 - THRESHOLD_EPOCH)
            .and_then(|index| snapshot.get_epoch_ext(&index))
            .expect("get_epoch_ext")
            .last_block_hash_in_previous_epoch();

        let frozen_number = freezer.number();

        let threshold = cmp::min(
            snapshot
                .get_block_number(&limit_block_hash)
                .expect("get_block_number"),
            frozen_number + MAX_FREEZE_LIMIT,
        );

        ckb_logger::trace!(
            "freezer current_epoch {} number {} threshold {}",
            current_epoch,
            frozen_number,
            threshold
        );

        let store = self.store();
        let get_unfrozen_block = |number: BlockNumber| {
            store
                .get_block_hash(number)
                .and_then(|hash| store.get_unfrozen_block(&hash))
        };

        let ret = freezer.freeze(threshold, get_unfrozen_block)?;

        let stopped = freezer.stopped.load(Ordering::SeqCst);

        // Wipe out frozen data
        self.wipe_out_frozen_data(&snapshot, ret, stopped)?;

        ckb_logger::trace!("freezer finish");

        Ok(())
    }

    fn wipe_out_frozen_data(
        &self,
        snapshot: &Snapshot,
        frozen: BTreeMap<packed::Byte32, (BlockNumber, u32)>,
        stopped: bool,
    ) -> Result<(), Error> {
        let mut side = BTreeMap::new();
        let mut batch = self.store.new_write_batch();

        ckb_logger::trace!("freezer wipe_out_frozen_data {} ", frozen.len());

        if !frozen.is_empty() {
            // remain header
            for (hash, (number, txs)) in &frozen {
                batch.delete_block_body(*number, hash, *txs).map_err(|e| {
                    ckb_logger::error!("freezer delete_block_body failed {}", e);
                    e
                })?;

                let pack_number: packed::Uint64 = number.pack();
                let prefix = pack_number.as_slice();
                for (key, value) in snapshot
                    .get_iter(
                        COLUMN_NUMBER_HASH,
                        IteratorMode::From(prefix, Direction::Forward),
                    )
                    .take_while(|(key, _)| key.starts_with(prefix))
                {
                    let reader =
                        packed::NumberHashReader::from_slice_should_be_ok(&key.as_ref()[..]);
                    let block_hash = reader.block_hash().to_entity();
                    if &block_hash != hash {
                        let txs =
                            packed::Uint32Reader::from_slice_should_be_ok(&value.as_ref()[..])
                                .unpack();
                        side.insert(block_hash, (reader.number().to_entity(), txs));
                    }
                }
            }
            self.store.write_sync(&batch).map_err(|e| {
                ckb_logger::error!("freezer write_batch delete failed {}", e);
                e
            })?;
            batch.clear()?;

            if !stopped {
                let start = frozen.keys().min().expect("frozen empty checked");
                let end = frozen.keys().max().expect("frozen empty checked");
                self.compact_block_body(start, end);
            }
        }

        if !side.is_empty() {
            // Wipe out side chain
            for (hash, (number, txs)) in &side {
                batch
                    .delete_block(number.unpack(), hash, *txs)
                    .map_err(|e| {
                        ckb_logger::error!("freezer delete_block_body failed {}", e);
                        e
                    })?;
            }

            self.store.write(&batch).map_err(|e| {
                ckb_logger::error!("freezer write_batch delete failed {}", e);
                e
            })?;

            if !stopped {
                let start = side.keys().min().expect("side empty checked");
                let end = side.keys().max().expect("side empty checked");
                self.compact_block_body(start, end);
            }
        }
        Ok(())
    }

    fn compact_block_body(&self, start: &packed::Byte32, end: &packed::Byte32) {
        let start_t = packed::TransactionKey::new_builder()
            .block_hash(start.clone())
            .index(0u32.pack())
            .build();

        let end_t = packed::TransactionKey::new_builder()
            .block_hash(end.clone())
            .index(TX_INDEX_UPPER_BOUND.pack())
            .build();

        if let Err(e) = self.store.compact_range(
            COLUMN_BLOCK_BODY,
            Some(start_t.as_slice()),
            Some(end_t.as_slice()),
        ) {
            ckb_logger::error!("freezer compact_range {}-{} error {}", start, end, e);
        }
    }

    /// TODO(doc): @quake
    pub fn tx_pool_controller(&self) -> &TxPoolController {
        &self.tx_pool_controller
    }

    /// TODO(doc): @quake
    pub fn txs_verify_cache(&self) -> Arc<TokioRwLock<TxVerifyCache>> {
        Arc::clone(&self.txs_verify_cache)
    }

    /// TODO(doc): @quake
    pub fn notify_controller(&self) -> &NotifyController {
        &self.notify_controller
    }

    /// TODO(doc): @quake
    pub fn snapshot(&self) -> Guard<Arc<Snapshot>> {
        self.snapshot_mgr.load()
    }

    /// TODO(doc): @quake
    pub fn store_snapshot(&self, snapshot: Arc<Snapshot>) {
        self.snapshot_mgr.store(snapshot)
    }

    /// TODO(doc): @quake
    pub fn refresh_snapshot(&self) {
        let new = self.snapshot().refresh(self.store.get_snapshot());
        self.store_snapshot(Arc::new(new));
    }

    /// TODO(doc): @quake
    pub fn new_snapshot(
        &self,
        tip_header: HeaderView,
        total_difficulty: U256,
        epoch_ext: EpochExt,
        proposals: ProposalView,
    ) -> Arc<Snapshot> {
        Arc::new(Snapshot::new(
            tip_header,
            total_difficulty,
            epoch_ext,
            self.store.get_snapshot(),
            proposals,
            Arc::clone(&self.consensus),
        ))
    }

    /// TODO(doc): @quake
    pub fn consensus(&self) -> &Consensus {
        &self.consensus
    }

    /// Return async runtime handle
    pub fn async_handle(&self) -> &Handle {
        &self.async_handle
    }

    /// TODO(doc): @quake
    pub fn genesis_hash(&self) -> Byte32 {
        self.consensus.genesis_hash()
    }

    /// TODO(doc): @quake
    pub fn store(&self) -> &ChainDB {
        &self.store
    }

    /// Return whether chain is in initial block download
    pub fn is_initial_block_download(&self) -> bool {
        // Once this function has returned false, it must remain false.
        if self.ibd_finished.load(Ordering::Relaxed) {
            false
        } else if unix_time_as_millis().saturating_sub(self.snapshot().tip_header().timestamp())
            > MAX_TIP_AGE
        {
            true
        } else {
            self.ibd_finished.store(true, Ordering::Relaxed);
            false
        }
    }
}

/// TODO(doc): @quake
pub struct SharedBuilder {
    db: RocksDB,
    ancient_path: Option<PathBuf>,
    consensus: Option<Consensus>,
    tx_pool_config: Option<TxPoolConfig>,
    store_config: Option<StoreConfig>,
    block_assembler_config: Option<BlockAssemblerConfig>,
    notify_config: Option<NotifyConfig>,
    async_handle: Handle,
    // async stop handle, only test will be assigned
    async_stop: Option<StopHandler<()>>,
}

impl SharedBuilder {
    /// Generates the base SharedBuilder with ancient path and async_handle
    pub fn new(db_config: &DBConfig, async_handle: Handle) -> Self {
        SharedBuilder {
            db: RocksDB::open(db_config, COLUMNS),
            ancient_path: None,
            consensus: None,
            tx_pool_config: None,
            notify_config: None,
            store_config: None,
            block_assembler_config: None,
            async_handle,
            async_stop: None,
        }
    }

    /// Generates the SharedBuilder with temp db
    pub fn with_temp_db() -> Self {
        let (handle, stop) = new_global_runtime();
        SharedBuilder {
            db: RocksDB::open_tmp(COLUMNS),
            ancient_path: None,
            consensus: None,
            tx_pool_config: None,
            notify_config: None,
            store_config: None,
            block_assembler_config: None,
            async_handle: handle,
            async_stop: Some(stop),
        }
    }
}

impl SharedBuilder {
    /// TODO(doc): @quake
    pub fn consensus(mut self, value: Consensus) -> Self {
        self.consensus = Some(value);
        self
    }

    /// TODO(doc): @quake
    pub fn tx_pool_config(mut self, config: TxPoolConfig) -> Self {
        self.tx_pool_config = Some(config);
        self
    }

    /// TODO(doc): @quake
    pub fn notify_config(mut self, config: NotifyConfig) -> Self {
        self.notify_config = Some(config);
        self
    }

    /// TODO(doc): @quake
    pub fn store_config(mut self, config: StoreConfig) -> Self {
        self.store_config = Some(config);
        self
    }

    /// TODO(doc): @quake
    pub fn block_assembler_config(mut self, config: Option<BlockAssemblerConfig>) -> Self {
        self.block_assembler_config = config;
        self
    }

    /// specifies the async_handle for the shared
    pub fn async_handle(mut self, async_handle: Handle) -> Self {
        self.async_handle = async_handle;
        self
    }

    /// TODO(doc): @quake
    pub fn build(self) -> Result<(Shared, ProposalTable), Error> {
        let consensus = self.consensus.unwrap_or_else(Consensus::default);
        let tx_pool_config = self.tx_pool_config.unwrap_or_else(Default::default);
        let notify_config = self.notify_config.unwrap_or_else(Default::default);
        let store_config = self.store_config.unwrap_or_else(Default::default);

        let store = if store_config.freezer_enable && self.ancient_path.is_some() {
            let freezer = Freezer::open(self.ancient_path.expect("exist checked"))?;
            ChainDB::new_with_freezer(self.db, freezer, store_config)
        } else {
            ChainDB::new(self.db, store_config)
        };

        Shared::init(
            store,
            consensus,
            tx_pool_config,
            notify_config,
            self.block_assembler_config,
            self.async_handle,
            self.async_stop,
        )
    }
}
