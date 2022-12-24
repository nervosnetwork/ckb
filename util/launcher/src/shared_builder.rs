//! Shared factory
//!
//! which can be used in order to configure the properties of a new shared.

use crate::migrate::Migrate;
use ckb_app_config::ExitCode;
use ckb_app_config::{BlockAssemblerConfig, DBConfig, NotifyConfig, StoreConfig, TxPoolConfig};
use ckb_async_runtime::{new_background_runtime, Handle};
use ckb_chain_spec::consensus::Consensus;
use ckb_chain_spec::SpecError;
use ckb_channel::Receiver;
use ckb_db::RocksDB;
use ckb_db_schema::COLUMNS;
use ckb_error::{Error, InternalErrorKind};
use ckb_freezer::Freezer;
use ckb_logger::{error, info};
use ckb_notify::{NotifyController, NotifyService, PoolTransactionEntry};
use ckb_proposal_table::ProposalTable;
use ckb_proposal_table::ProposalView;
use ckb_shared::Shared;
use ckb_snapshot::{Snapshot, SnapshotMgr};
use ckb_stop_handler::StopHandler;
use ckb_store::ChainDB;
use ckb_store::ChainStore;
use ckb_tx_pool::{
    error::Reject, service::TxVerificationResult, TokioRwLock, TxEntry, TxPool,
    TxPoolServiceBuilder,
};
use ckb_types::core::EpochExt;
use ckb_types::core::HeaderView;
use ckb_verification::cache::init_cache;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tempfile::TempDir;

/// Shared builder for construct new shared.
pub struct SharedBuilder {
    db: RocksDB,
    ancient_path: Option<PathBuf>,
    consensus: Option<Consensus>,
    tx_pool_config: Option<TxPoolConfig>,
    store_config: Option<StoreConfig>,
    block_assembler_config: Option<BlockAssemblerConfig>,
    notify_config: Option<NotifyConfig>,
    async_handle: Handle,
}

pub fn open_or_create_db(
    bin_name: &str,
    root_dir: &Path,
    config: &DBConfig,
) -> Result<RocksDB, ExitCode> {
    let migrate = Migrate::new(&config.path);

    let read_only_db = migrate.open_read_only_db().map_err(|e| {
        eprintln!("migrate error {}", e);
        ExitCode::Failure
    })?;

    if let Some(db) = read_only_db {
        match migrate.check(&db) {
            Ordering::Greater => {
                eprintln!(
                    "The database is created by a higher version CKB executable binary, \n\
                     so that the current CKB executable binary couldn't open this database.\n\
                     Please download the latest CKB executable binary."
                );
                Err(ExitCode::Failure)
            }
            Ordering::Equal => Ok(RocksDB::open(config, COLUMNS)),
            Ordering::Less => {
                if migrate.require_expensive(&db) {
                    eprintln!(
                        "For optimal performance, CKB wants to migrate the data into new format.\n\
                        You can use the old version CKB if you don't want to do the migration.\n\
                        We strongly recommended you to use the latest stable version of CKB, \
                        since the old versions may have unfixed vulnerabilities.\n\
                        Run `\"{}\" migrate -C \"{}\"` and confirm by typing \"YES\" to migrate the data.\n\
                        We strongly recommend that you backup the data directory before migration.",
                        bin_name,
                        root_dir.display()
                    );
                    Err(ExitCode::Failure)
                } else {
                    info!("process fast migrations ...");

                    let bulk_load_db_db = migrate.open_bulk_load_db().map_err(|e| {
                        eprintln!("migrate error {}", e);
                        ExitCode::Failure
                    })?;

                    if let Some(db) = bulk_load_db_db {
                        migrate.migrate(db).map_err(|err| {
                            eprintln!("Run error: {:?}", err);
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
            eprintln!("migrate init_db_version error {}", e);
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
    ) -> Result<SharedBuilder, ExitCode> {
        let db = open_or_create_db(bin_name, root_dir, db_config)?;

        Ok(SharedBuilder {
            db,
            ancient_path: ancient,
            consensus: None,
            tx_pool_config: None,
            notify_config: None,
            store_config: None,
            block_assembler_config: None,
            async_handle,
        })
    }

    /// Generates the SharedBuilder with temp db
    pub fn with_temp_db() -> Self {
        use once_cell::{sync, unsync};
        use std::{
            borrow::Borrow,
            sync::atomic::{AtomicUsize, Ordering},
        };

        // once #[thread_local] is stable
        // #[thread_local]
        // static RUNTIME_HANDLE: unsync::OnceCell<...

        thread_local! {
            static TMP_DIR: sync::OnceCell<TempDir> = sync::OnceCell::new();
            // NOTICE：we can't put the runtime directly into thread_local here,
            // on windows the runtime in thread_local will get stuck when dropping
            static RUNTIME_HANDLE: unsync::OnceCell<(Handle, StopHandler<()>)> = unsync::OnceCell::new();
        }

        static DB_COUNT: AtomicUsize = AtomicUsize::new(0);

        let db = {
            let db_id = DB_COUNT.fetch_add(1, Ordering::SeqCst);
            let db_base_dir = TMP_DIR.with(|tmp_dir| {
                tmp_dir
                    .borrow()
                    .get_or_try_init(TempDir::new)
                    .unwrap()
                    .path()
                    .to_path_buf()
            });
            let db_dir = db_base_dir.join(format!("db_{}", db_id));
            RocksDB::open_in(db_dir, COLUMNS)
        };

        RUNTIME_HANDLE.with(|runtime| SharedBuilder {
            db,
            ancient_path: None,
            consensus: None,
            tx_pool_config: None,
            notify_config: None,
            store_config: None,
            block_assembler_config: None,
            async_handle: runtime
                .borrow()
                .get_or_init(new_background_runtime)
                .0
                .clone(),
        })
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
            block_assembler_config,
            notify_config,
            async_handle,
        } = self;

        let tx_pool_config = tx_pool_config.unwrap_or_default();
        let notify_config = notify_config.unwrap_or_default();
        let store_config = store_config.unwrap_or_default();
        let consensus = Arc::new(consensus.unwrap_or_default());

        let notify_controller = start_notify_service(notify_config, async_handle.clone());

        let store = build_store(db, store_config, ancient_path).map_err(|e| {
            eprintln!("build_store {}", e);
            ExitCode::Failure
        })?;

        let txs_verify_cache = Arc::new(TokioRwLock::new(init_cache()));

        let (snapshot, table) =
            Self::init_snapshot(&store, Arc::clone(&consensus)).map_err(|e| {
                eprintln!("init_snapshot {}", e);
                ExitCode::Failure
            })?;
        let snapshot = Arc::new(snapshot);
        let snapshot_mgr = Arc::new(SnapshotMgr::new(Arc::clone(&snapshot)));

        let (sender, receiver) = ckb_channel::unbounded();

        let (mut tx_pool_builder, tx_pool_controller) = TxPoolServiceBuilder::new(
            tx_pool_config,
            Arc::clone(&snapshot),
            block_assembler_config,
            Arc::clone(&txs_verify_cache),
            &async_handle,
            sender,
        );

        register_tx_pool_callback(&mut tx_pool_builder, notify_controller.clone());

        let ibd_finished = Arc::new(AtomicBool::new(false));
        let shared = Shared::new(
            store,
            tx_pool_controller,
            notify_controller,
            txs_verify_cache,
            consensus,
            snapshot_mgr,
            async_handle,
            ibd_finished,
        );

        let pack = SharedPackage {
            table: Some(table),
            tx_pool_builder: Some(tx_pool_builder),
            relay_tx_receiver: Some(receiver),
        };

        Ok((shared, pack))
    }
}

/// SharedBuilder build returning the shared/package halves
/// The package structs used for init other component
pub struct SharedPackage {
    table: Option<ProposalTable>,
    tx_pool_builder: Option<TxPoolServiceBuilder>,
    relay_tx_receiver: Option<Receiver<TxVerificationResult>>,
}

impl SharedPackage {
    /// Takes the proposal_table out of the package, leaving a None in its place.
    pub fn take_proposal_table(&mut self) -> ProposalTable {
        self.table.take().expect("take proposal_table")
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

fn register_tx_pool_callback(tx_pool_builder: &mut TxPoolServiceBuilder, notify: NotifyController) {
    let notify_pending = notify.clone();
    tx_pool_builder.register_pending(Box::new(move |tx_pool: &mut TxPool, entry: &TxEntry| {
        // update statics
        tx_pool.update_statics_for_add_tx(entry.size, entry.cycles);

        // notify
        let notify_tx_entry = PoolTransactionEntry {
            transaction: entry.rtx.transaction.clone(),
            cycles: entry.cycles,
            size: entry.size,
            fee: entry.fee,
            timestamp: entry.timestamp,
        };
        notify_pending.notify_new_transaction(notify_tx_entry);
    }));

    let notify_proposed = notify.clone();
    tx_pool_builder.register_proposed(Box::new(
        move |tx_pool: &mut TxPool, entry: &TxEntry, new: bool| {
            // update statics
            if new {
                tx_pool.update_statics_for_add_tx(entry.size, entry.cycles);
            }

            // notify
            let notify_tx_entry = PoolTransactionEntry {
                transaction: entry.rtx.transaction.clone(),
                cycles: entry.cycles,
                size: entry.size,
                fee: entry.fee,
                timestamp: entry.timestamp,
            };
            notify_proposed.notify_proposed_transaction(notify_tx_entry);
        },
    ));

    tx_pool_builder.register_committed(Box::new(move |tx_pool: &mut TxPool, entry: &TxEntry| {
        tx_pool.update_statics_for_remove_tx(entry.size, entry.cycles);
    }));

    let notify_reject = notify;
    tx_pool_builder.register_reject(Box::new(
        move |tx_pool: &mut TxPool, entry: &TxEntry, reject: Reject| {
            // update statics
            tx_pool.update_statics_for_remove_tx(entry.size, entry.cycles);

            let tx_hash = entry.transaction().hash();
            // record recent reject
            if matches!(reject, Reject::Resolve(..)) {
                if let Some(ref mut recent_reject) = tx_pool.recent_reject {
                    if let Err(e) = recent_reject.put(&tx_hash, reject.clone()) {
                        error!("record recent_reject failed {} {} {}", tx_hash, reject, e);
                    }
                }
            }

            // notify
            let notify_tx_entry = PoolTransactionEntry {
                transaction: entry.rtx.transaction.clone(),
                cycles: entry.cycles,
                size: entry.size,
                fee: entry.fee,
                timestamp: entry.timestamp,
            };
            notify_reject.notify_reject_transaction(notify_tx_entry, reject);
        },
    ));
}
