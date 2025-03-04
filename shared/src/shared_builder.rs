//! shared_builder provide SharedBuilder and SharedPacakge
use crate::ChainServicesBuilder;
use crate::{HeaderMap, Shared};
use ckb_app_config::{
    BlockAssemblerConfig, DBConfig, ExitCode, FeeEstimatorAlgo, FeeEstimatorConfig, NotifyConfig,
    StoreConfig, SyncConfig, TxPoolConfig,
};
use ckb_async_runtime::{Handle, new_background_runtime};
use ckb_chain_spec::SpecError;
use ckb_chain_spec::consensus::Consensus;
use ckb_channel::Receiver;
use ckb_db::RocksDB;
use ckb_db_schema::COLUMNS;
use ckb_error::{Error, InternalErrorKind};
use ckb_fee_estimator::FeeEstimator;
use ckb_logger::{error, info};
use ckb_migrate::migrate::Migrate;
use ckb_notify::{NotifyController, NotifyService};
use ckb_proposal_table::ProposalTable;
use ckb_proposal_table::ProposalView;
use ckb_snapshot::{Snapshot, SnapshotMgr};
use ckb_store::{ChainDB, ChainStore, Freezer};
use ckb_tx_pool::{
    TokioRwLock, TxEntry, TxPool, TxPoolServiceBuilder, service::TxVerificationResult,
};
use ckb_types::H256;
use ckb_types::core::hardfork::HardForks;
use ckb_types::prelude::Pack;
use ckb_types::{
    core::EpochExt, core::HeaderView, core::service::PoolTransactionEntry, core::tx_pool::Reject,
};
use ckb_util::Mutex;
use ckb_verification::cache::init_cache;
use dashmap::DashMap;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tempfile::TempDir;

/// Shared builder for construct new shared.
pub struct SharedBuilder {
    db: RocksDB,
    ancient_path: Option<PathBuf>,
    consensus: Consensus,
    tx_pool_config: Option<TxPoolConfig>,
    store_config: Option<StoreConfig>,
    sync_config: Option<SyncConfig>,
    block_assembler_config: Option<BlockAssemblerConfig>,
    notify_config: Option<NotifyConfig>,
    async_handle: Handle,
    fee_estimator_config: Option<FeeEstimatorConfig>,

    header_map_tmp_dir: Option<PathBuf>,
}

/// Open or create a rocksdb
pub fn open_or_create_db(
    bin_name: &str,
    root_dir: &Path,
    config: &DBConfig,
    hardforks: HardForks,
) -> Result<RocksDB, ExitCode> {
    let migrate = Migrate::new(&config.path, hardforks);

    let read_only_db = migrate.open_read_only_db().map_err(|e| {
        eprintln!("Migration error {e}");
        ExitCode::Failure
    })?;

    if let Some(db) = read_only_db {
        match migrate.check(&db, true) {
            Ordering::Greater => {
                eprintln!(
                    "The database was created by a higher version CKB executable binary \n\
                     and cannot be opened by the current binary.\n\
                     Please download the latest CKB executable binary."
                );
                Err(ExitCode::Failure)
            }
            Ordering::Equal => Ok(RocksDB::open(config, COLUMNS)),
            Ordering::Less => {
                let can_run_in_background = migrate.can_run_in_background(&db);
                if migrate.require_expensive(&db, false) && !can_run_in_background {
                    eprintln!(
                        "For optimal performance, CKB recommends migrating your data into a new format.\n\
                        If you prefer to stick with the older version, \n\
                        it's important to note that they may have unfixed vulnerabilities.\n\
                        Before migrating, we strongly recommend backuping your data directory.\n\
                        To migrate, run `\"{}\" migrate -C \"{}\"` and confirm by typing \"YES\".",
                        bin_name,
                        root_dir.display()
                    );
                    Err(ExitCode::Failure)
                } else if can_run_in_background {
                    info!("process migrations in background ...");
                    let db = RocksDB::open(config, COLUMNS);
                    migrate.migrate(db.clone(), true).map_err(|err| {
                        eprintln!("Run error: {err:?}");
                        ExitCode::Failure
                    })?;
                    Ok(db)
                } else {
                    info!("Processing fast migrations ...");

                    let bulk_load_db_db = migrate.open_bulk_load_db().map_err(|e| {
                        eprintln!("Migration error {e}");
                        ExitCode::Failure
                    })?;

                    if let Some(db) = bulk_load_db_db {
                        migrate.migrate(db, false).map_err(|err| {
                            eprintln!("Run error: {err:?}");
                            ExitCode::Failure
                        })?;
                    }

                    Ok(RocksDB::open(config, COLUMNS))
                }
            }
        }
    } else {
        let db = RocksDB::open(config, COLUMNS);
        migrate.init_db_version(&db).map_err(|e| {
            eprintln!("Migrate init_db_version error {e}");
            ExitCode::Failure
        })?;
        Ok(db)
    }
}

impl SharedBuilder {
    /// Generates the base SharedBuilder with ancient path and async_handle
    pub fn new(
        bin_name: &str,
        root_dir: &Path,
        db_config: &DBConfig,
        ancient: Option<PathBuf>,
        async_handle: Handle,
        consensus: Consensus,
    ) -> Result<SharedBuilder, ExitCode> {
        let db = open_or_create_db(
            bin_name,
            root_dir,
            db_config,
            consensus.hardfork_switch.clone(),
        )?;

        Ok(SharedBuilder {
            db,
            ancient_path: ancient,
            consensus,
            tx_pool_config: None,
            notify_config: None,
            store_config: None,
            sync_config: None,
            block_assembler_config: None,
            async_handle,
            fee_estimator_config: None,
            header_map_tmp_dir: None,
        })
    }

    /// Generates the SharedBuilder with temp db
    /// NOTICE: this is only used in testing
    pub fn with_temp_db() -> Self {
        use std::{
            borrow::Borrow,
            sync::atomic::{AtomicUsize, Ordering},
        };

        // once #[thread_local] is stable
        // #[thread_local]
        // static RUNTIME_HANDLE: unsync::OnceCell<...
        thread_local! {
            // NOTICE：we can't put the runtime directly into thread_local here,
            // on windows the runtime in thread_local will get stuck when dropping
            static RUNTIME_HANDLE: std::cell::OnceCell<Handle> = const { std::cell::OnceCell::new() };
        }

        static DB_COUNT: AtomicUsize = AtomicUsize::new(0);
        static TMP_DIR: std::sync::OnceLock<TempDir> = std::sync::OnceLock::new();

        let db = {
            let db_id = DB_COUNT.fetch_add(1, Ordering::SeqCst);
            let db_base_dir = TMP_DIR
                .borrow()
                .get_or_init(|| TempDir::new().unwrap())
                .path()
                .to_path_buf();
            let db_dir = db_base_dir.join(format!("db_{db_id}"));
            RocksDB::open_in(db_dir, COLUMNS)
        };

        RUNTIME_HANDLE.with(|runtime| SharedBuilder {
            db,
            ancient_path: None,
            consensus: Consensus::default(),
            tx_pool_config: None,
            notify_config: None,
            store_config: None,
            sync_config: None,
            block_assembler_config: None,
            async_handle: runtime.get_or_init(new_background_runtime).clone(),
            fee_estimator_config: None,

            header_map_tmp_dir: None,
        })
    }
}

impl SharedBuilder {
    /// TODO(doc): @quake
    pub fn consensus(mut self, value: Consensus) -> Self {
        self.consensus = value;
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

    /// TODO(doc): @eval-exec
    pub fn sync_config(mut self, config: SyncConfig) -> Self {
        self.sync_config = Some(config);
        self
    }

    /// TODO(doc): @eval-exec
    pub fn header_map_tmp_dir(mut self, header_map_tmp_dir: Option<PathBuf>) -> Self {
        self.header_map_tmp_dir = header_map_tmp_dir;
        self
    }

    /// TODO(doc): @quake
    pub fn block_assembler_config(mut self, config: Option<BlockAssemblerConfig>) -> Self {
        self.block_assembler_config = config;
        self
    }

    /// Sets the configuration for the fee estimator.
    pub fn fee_estimator_config(mut self, config: FeeEstimatorConfig) -> Self {
        self.fee_estimator_config = Some(config);
        self
    }

    /// specifies the async_handle for the shared
    pub fn async_handle(mut self, async_handle: Handle) -> Self {
        self.async_handle = async_handle;
        self
    }

    fn init_proposal_table(
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

    fn init_store(store: &ChainDB, consensus: &Consensus) -> Result<(HeaderView, EpochExt), Error> {
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
            None => store.init(consensus).map(|_| {
                (
                    consensus.genesis_block().header(),
                    consensus.genesis_epoch_ext().to_owned(),
                )
            }),
        }
    }

    fn init_snapshot(
        store: &ChainDB,
        consensus: Arc<Consensus>,
    ) -> Result<(Snapshot, ProposalTable), Error> {
        let (tip_header, epoch) = Self::init_store(store, &consensus)?;
        let total_difficulty = store
            .get_block_ext(&tip_header.hash())
            .ok_or_else(|| InternalErrorKind::Database.other("failed to get tip's block_ext"))?
            .total_difficulty;
        let (proposal_table, proposal_view) = Self::init_proposal_table(store, &consensus);

        let snapshot = Snapshot::new(
            tip_header,
            total_difficulty,
            epoch,
            store.get_snapshot(),
            proposal_view,
            consensus,
        );

        Ok((snapshot, proposal_table))
    }

    /// TODO(doc): @quake
    pub fn build(self) -> Result<(Shared, SharedPackage), ExitCode> {
        let SharedBuilder {
            db,
            ancient_path,
            consensus,
            tx_pool_config,
            store_config,
            sync_config,
            block_assembler_config,
            notify_config,
            async_handle,
            fee_estimator_config,
            header_map_tmp_dir,
        } = self;

        let tx_pool_config = tx_pool_config.unwrap_or_default();
        let notify_config = notify_config.unwrap_or_default();
        let store_config = store_config.unwrap_or_default();
        let sync_config = sync_config.unwrap_or_default();
        let consensus = Arc::new(consensus);

        let header_map_memory_limit = sync_config.header_map.memory_limit.as_u64() as usize;

        let ibd_finished = Arc::new(AtomicBool::new(false));

        let header_map = Arc::new(HeaderMap::new(
            header_map_tmp_dir,
            header_map_memory_limit,
            &async_handle,
            Arc::clone(&ibd_finished),
        ));

        let notify_controller = start_notify_service(notify_config, async_handle.clone());

        let store = build_store(db, store_config, ancient_path).map_err(|e| {
            eprintln!("build_store {e}");
            ExitCode::Failure
        })?;

        let txs_verify_cache = Arc::new(TokioRwLock::new(init_cache()));

        let (snapshot, table) =
            Self::init_snapshot(&store, Arc::clone(&consensus)).map_err(|e| {
                eprintln!("init_snapshot {e}");
                ExitCode::Failure
            })?;
        let snapshot = Arc::new(snapshot);
        let snapshot_mgr = Arc::new(SnapshotMgr::new(Arc::clone(&snapshot)));

        let (sender, receiver) = ckb_channel::unbounded();

        let fee_estimator_algo = fee_estimator_config
            .map(|config| config.algorithm)
            .unwrap_or(None);
        let fee_estimator = match fee_estimator_algo {
            Some(FeeEstimatorAlgo::WeightUnitsFlow) => FeeEstimator::new_weight_units_flow(),
            Some(FeeEstimatorAlgo::ConfirmationFraction) => {
                FeeEstimator::new_confirmation_fraction()
            }
            None => FeeEstimator::new_dummy(),
        };

        let (mut tx_pool_builder, tx_pool_controller) = TxPoolServiceBuilder::new(
            tx_pool_config,
            Arc::clone(&snapshot),
            block_assembler_config,
            Arc::clone(&txs_verify_cache),
            &async_handle,
            sender,
            fee_estimator.clone(),
        );

        register_tx_pool_callback(
            &mut tx_pool_builder,
            notify_controller.clone(),
            fee_estimator,
        );

        let block_status_map = Arc::new(DashMap::new());

        let assume_valid_targets = Arc::new(Mutex::new({
            let not_exists_targets: Option<Vec<H256>> =
                sync_config.assume_valid_targets.clone().map(|targets| {
                    targets
                        .iter()
                        .filter(|&target_hash| {
                            let exists = snapshot.block_exists(&target_hash.pack());
                            if exists {
                                info!("assume-valid target 0x{} exists in local db", target_hash);
                            }
                            !exists
                        })
                        .cloned()
                        .collect::<Vec<H256>>()
                });

            if not_exists_targets
                .as_ref()
                .is_some_and(|targets| targets.is_empty())
            {
                info!("all assume-valid targets synchronized, enter full verification mode");
                None
            } else {
                not_exists_targets
            }
        }));

        let assume_valid_target_specified: Arc<Option<H256>> = Arc::new(
            sync_config
                .assume_valid_targets
                .and_then(|targets| targets.last().cloned()),
        );

        let shared = Shared::new(
            store,
            tx_pool_controller,
            notify_controller,
            txs_verify_cache,
            consensus,
            snapshot_mgr,
            async_handle,
            ibd_finished,
            assume_valid_targets,
            assume_valid_target_specified,
            header_map,
            block_status_map,
        );

        let chain_services_builder = ChainServicesBuilder::new(shared.clone(), table);

        let pack = SharedPackage {
            chain_services_builder: Some(chain_services_builder),
            tx_pool_builder: Some(tx_pool_builder),
            relay_tx_receiver: Some(receiver),
        };

        Ok((shared, pack))
    }
}

/// SharedBuilder build returning the shared/package halves
/// The package structs used for init other component
pub struct SharedPackage {
    chain_services_builder: Option<ChainServicesBuilder>,
    tx_pool_builder: Option<TxPoolServiceBuilder>,
    relay_tx_receiver: Option<Receiver<TxVerificationResult>>,
}

impl SharedPackage {
    /// Takes the chain_services_builder out of the package, leaving a None in its place.
    pub fn take_chain_services_builder(&mut self) -> ChainServicesBuilder {
        self.chain_services_builder
            .take()
            .expect("take chain_services_builder")
    }

    /// Takes the tx_pool_builder out of the package, leaving a None in its place.
    pub fn take_tx_pool_builder(&mut self) -> TxPoolServiceBuilder {
        self.tx_pool_builder.take().expect("take tx_pool_builder")
    }

    /// Takes the relay_tx_receiver out of the package, leaving a None in its place.
    pub fn take_relay_tx_receiver(&mut self) -> Receiver<TxVerificationResult> {
        self.relay_tx_receiver
            .take()
            .expect("take relay_tx_receiver")
    }
}

fn start_notify_service(notify_config: NotifyConfig, handle: Handle) -> NotifyController {
    NotifyService::new(notify_config, handle).start()
}

fn build_store(
    db: RocksDB,
    store_config: StoreConfig,
    ancient_path: Option<PathBuf>,
) -> Result<ChainDB, Error> {
    let store = if store_config.freezer_enable && ancient_path.is_some() {
        let freezer = Freezer::open(ancient_path.expect("exist checked"))?;
        ChainDB::new_with_freezer(db, freezer, store_config)
    } else {
        ChainDB::new(db, store_config)
    };
    Ok(store)
}

fn register_tx_pool_callback(
    tx_pool_builder: &mut TxPoolServiceBuilder,
    notify: NotifyController,
    fee_estimator: FeeEstimator,
) {
    let notify_pending = notify.clone();

    let tx_relay_sender = tx_pool_builder.tx_relay_sender();
    let create_notify_entry = |entry: &TxEntry| PoolTransactionEntry {
        transaction: entry.rtx.transaction.clone(),
        cycles: entry.cycles,
        size: entry.size,
        fee: entry.fee,
        timestamp: entry.timestamp,
    };

    let fee_estimator_clone = fee_estimator.clone();
    tx_pool_builder.register_pending(Box::new(move |entry: &TxEntry| {
        // notify
        let notify_tx_entry = create_notify_entry(entry);
        notify_pending.notify_new_transaction(notify_tx_entry);
        let tx_hash = entry.transaction().hash();
        let entry_info = entry.to_info();
        fee_estimator_clone.accept_tx(tx_hash, entry_info);
    }));

    let notify_proposed = notify.clone();
    tx_pool_builder.register_proposed(Box::new(move |entry: &TxEntry| {
        // notify
        let notify_tx_entry = create_notify_entry(entry);
        notify_proposed.notify_proposed_transaction(notify_tx_entry);
    }));

    let notify_reject = notify;
    tx_pool_builder.register_reject(Box::new(
        move |tx_pool: &mut TxPool, entry: &TxEntry, reject: Reject| {
            let tx_hash = entry.transaction().hash();
            // record recent reject
            if reject.should_recorded() {
                if let Some(ref mut recent_reject) = tx_pool.recent_reject {
                    if let Err(e) = recent_reject.put(&tx_hash, reject.clone()) {
                        error!("record recent_reject failed {} {} {}", tx_hash, reject, e);
                    }
                }
            }

            if reject.is_allowed_relay() {
                if let Err(e) = tx_relay_sender.send(TxVerificationResult::Reject {
                    tx_hash: tx_hash.clone(),
                }) {
                    error!("tx-pool tx_relay_sender internal error {}", e);
                }
            }

            // notify
            let notify_tx_entry = create_notify_entry(entry);
            notify_reject.notify_reject_transaction(notify_tx_entry, reject);

            // fee estimator
            fee_estimator.reject_tx(&tx_hash);
        },
    ));
}
