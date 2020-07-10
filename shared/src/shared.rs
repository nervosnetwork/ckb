//! TODO(doc): @quake
use crate::{migrations, Snapshot, SnapshotMgr};
use arc_swap::Guard;
use ckb_app_config::{BlockAssemblerConfig, DBConfig, NotifyConfig, StoreConfig, TxPoolConfig};
use ckb_async_runtime::{new_global_runtime, Handle};
use ckb_chain_spec::consensus::Consensus;
use ckb_chain_spec::SpecError;
use ckb_db::{Direction, IteratorMode, RocksDB};
use ckb_db_migration::{DefaultMigration, Migrations};
use ckb_db_schema::{COLUMNS, COLUMN_NUMBER_HASH};
use ckb_error::{Error, InternalErrorKind};
use ckb_freezer::Freezer;
use ckb_notify::{NotifyController, NotifyService};
use ckb_proposal_table::{ProposalTable, ProposalView};
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_store::{ChainDB, ChainStore};
use ckb_tx_pool::{TokioRwLock, TxPoolController, TxPoolServiceBuilder};
use ckb_types::{
    core::{service, BlockNumber, EpochExt, EpochNumber, HeaderView},
    packed::{self, Byte32},
    prelude::*,
    U256,
};
use ckb_verification::cache::TxVerifyCache;
use once_cell::sync::OnceCell;
use std::cmp;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const FREEZER_INTERVAL: Duration = Duration::from_secs(60);
const THRESHOLD_EPOCH: EpochNumber = 2;
const MAX_FREEZE_LIMIT: BlockNumber = 30_000;

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

        let tx_pool_builder = TxPoolServiceBuilder::new(
            tx_pool_config,
            Arc::clone(&snapshot),
            block_assembler_config,
            Arc::clone(&txs_verify_cache),
            Arc::clone(&snapshot_mgr),
            notify_controller.clone(),
        );

        let tx_pool_controller = tx_pool_builder.start(&async_handle);

        let shared = Shared {
            store,
            consensus,
            txs_verify_cache,
            snapshot_mgr,
            tx_pool_controller,
            notify_controller,
            async_handle,
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

    /// TODO(doc): @zhangsoledad
    pub fn spawn_freeze(&self) -> FreezerClose {
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
        let freezer = self.store.freezer().expect("freezer inited");
        FreezerClose {
            stopped: Arc::clone(&freezer.stopped),
            stop,
        }
    }

    fn freeze(&self) -> Result<(), Error> {
        let freezer = self.store.freezer().expect("freezer inited");
        let snapshot = self.snapshot();
        let current_epoch = snapshot.epoch_ext().number();

        ckb_logger::trace!("freezer current_epoch {}", current_epoch);

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

        let get_unfrozen_block = |number: BlockNumber| {
            self.store()
                .get_block_hash(number)
                .and_then(|hash| self.store().get_unfrozen_block(&hash))
        };

        let ret = freezer.freeze(threshold, get_unfrozen_block)?;

        // Wipe out frozen data
        self.wipe_out_frozen_data(&snapshot, ret)?;

        ckb_logger::trace!("freezer finish ");

        Ok(())
    }

    fn wipe_out_frozen_data(
        &self,
        snapshot: &Snapshot,
        frozen: Vec<(BlockNumber, packed::Byte32, u32)>,
    ) -> Result<(), Error> {
        let mut side = Vec::with_capacity(frozen.len());
        let mut batch = self.store.new_write_batch();

        ckb_logger::trace!("freezer wipe_out_frozen_data {} ", frozen.len());

        // remain header
        for (number, hash, txs) in frozen {
            batch.delete_block_body(number, &hash, txs).map_err(|e| {
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
                let reader = packed::NumberHashReader::from_slice_should_be_ok(&key.as_ref()[..]);
                let block_hash = reader.block_hash().to_entity();
                if block_hash != hash {
                    let txs =
                        packed::Uint32Reader::from_slice_should_be_ok(&value.as_ref()[..]).unpack();
                    side.push((reader.number().to_entity(), block_hash, txs));
                }
            }
        }
        self.store.write(&batch).map_err(|e| {
            ckb_logger::error!("freezer write_batch delete failed {}", e);
            e
        })?;
        batch.clear()?;

        // Wipe out side chain
        for (number, hash, txs) in side {
            batch
                .delete_block(number.unpack(), &hash, txs)
                .map_err(|e| {
                    ckb_logger::error!("freezer delete_block_body failed {}", e);
                    e
                })?;
        }

        self.store.write(&batch).map_err(|e| {
            ckb_logger::error!("freezer write_batch delete failed {}", e);
            e
        })?;
        Ok(())
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
    migrations: Migrations,
    async_handle: Handle,
}

static RUNTIME: OnceCell<(Handle, StopHandler<()>)> = OnceCell::new();

fn static_runtime_handle() -> Handle {
    let runtime = RUNTIME.get_or_init(new_global_runtime);
    runtime.0.clone()
}

const INIT_DB_VERSION: &str = "20191127135521";

impl SharedBuilder {
    pub fn new(config: &DBConfig, ancient: Option<PathBuf>, async_handle: Handle) -> Self {
        let db = RocksDB::open(config, COLUMNS);
        let mut migrations = Migrations::default();
        migrations.add_migration(Box::new(DefaultMigration::new(INIT_DB_VERSION)));
        migrations.add_migration(Box::new(migrations::ChangeMoleculeTableToStruct));
        migrations.add_migration(Box::new(migrations::CellMigration));

        SharedBuilder {
            db,
            ancient_path: ancient,
            consensus: None,
            tx_pool_config: None,
            notify_config: None,
            store_config: None,
            block_assembler_config: None,
            migrations,
            async_handle,
        }
    }

    pub fn with_temp_db() -> Self {
        SharedBuilder {
            db: RocksDB::open_tmp(COLUMNS),
            ancient_path: None,
            consensus: None,
            tx_pool_config: None,
            notify_config: None,
            store_config: None,
            block_assembler_config: None,
            migrations: Migrations::default(),
            async_handle: static_runtime_handle(),
        }
    }
}

impl SharedBuilder {
    /// Check whether database requires migration
    ///
    /// Return true if migration is required
    pub fn migration_check(&self) -> bool {
        self.migrations.check(&self.db)
    }

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
        let db = self.migrations.migrate(self.db)?;
        let store = if let Some(path) = self.ancient_path {
            let freezer = Freezer::open(path)?;
            ChainDB::new_with_freezer(db, freezer, store_config)
        } else {
            ChainDB::new(db, store_config)
        };

        Shared::init(
            store,
            consensus,
            tx_pool_config,
            notify_config,
            self.block_assembler_config,
            self.async_handle,
        )
    }
}
