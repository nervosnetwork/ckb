use crate::shared::Shared;
use crate::PeerIndex;
use crate::{Snapshot, SnapshotMgr};
use ckb_app_config::{BlockAssemblerConfig, DBConfig, NotifyConfig, StoreConfig, TxPoolConfig};
use ckb_async_runtime::{new_global_runtime, Handle};
use ckb_chain_spec::consensus::Consensus;
use ckb_channel::Receiver;
use ckb_db::RocksDB;
use ckb_db_schema::COLUMNS;
use ckb_error::Error;
use ckb_freezer::Freezer;
use ckb_notify::{NotifyController, NotifyService, PoolTransactionEntry};
use ckb_proposal_table::ProposalTable;
use ckb_stop_handler::StopHandler;
use ckb_store::ChainDB;
use ckb_tx_pool::{
    error::Reject, TokioRwLock, TxEntry, TxPool, TxPoolController, TxPoolServiceBuilder,
};
use ckb_types::packed::Byte32;
use ckb_verification::cache::TxVerifyCache;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

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
    pub fn new(db_config: &DBConfig, ancient: Option<PathBuf>, async_handle: Handle) -> Self {
        SharedBuilder {
            db: RocksDB::open(db_config, COLUMNS),
            ancient_path: ancient,
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
    pub fn build(self) -> Result<(Shared, SharedPackage), Error> {
        let SharedBuilder {
            db,
            ancient_path,
            consensus,
            tx_pool_config,
            store_config,
            block_assembler_config,
            notify_config,
            async_handle,
            async_stop,
        } = self;

        let tx_pool_config = tx_pool_config.unwrap_or_else(Default::default);
        let notify_config = notify_config.unwrap_or_else(Default::default);
        let store_config = store_config.unwrap_or_else(Default::default);
        let consensus = Arc::new(consensus.unwrap_or_else(Consensus::default));

        let notify_controller = start_notify_service(notify_config);

        let store = build_store(db, store_config, ancient_path)?;

        let txs_verify_cache = Arc::new(TokioRwLock::new(TxVerifyCache::new(
            tx_pool_config.max_verify_cache_size,
        )));

        let (snapshot, table) = Shared::init_snapshot(&store, Arc::clone(&consensus))?;
        let snapshot = Arc::new(snapshot);
        let snapshot_mgr = Arc::new(SnapshotMgr::new(Arc::clone(&snapshot)));

        let (tx_pool_builder, tx_pool_controller) = build_tx_pool(
            tx_pool_config,
            block_assembler_config,
            notify_controller.clone(),
            Arc::clone(&txs_verify_cache),
            Arc::clone(&snapshot),
            Arc::clone(&snapshot_mgr),
            &async_handle,
        );

        let (sender, receiver) = ckb_channel::unbounded();
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
            relay_tx_sender: sender,
        };

        let pack = SharedPackage {
            table: Some(table),
            tx_pool_builder: Some(tx_pool_builder),
            relay_tx_receiver: Some(receiver),
        };

        Ok((shared, pack))
    }
}

pub struct SharedPackage {
    table: Option<ProposalTable>,
    tx_pool_builder: Option<TxPoolServiceBuilder>,
    relay_tx_receiver: Option<Receiver<(PeerIndex, Byte32)>>,
}

impl SharedPackage {
    pub fn take_proposal_table(&mut self) -> ProposalTable {
        self.table.take().expect("take proposal_table")
    }

    pub fn take_tx_pool_builder(&mut self) -> TxPoolServiceBuilder {
        self.tx_pool_builder.take().expect("take tx_pool_builder")
    }

    pub fn take_relay_tx_receiver(&mut self) -> Receiver<(PeerIndex, Byte32)> {
        self.relay_tx_receiver
            .take()
            .expect("take relay_tx_receiver")
    }
}

fn start_notify_service(notify_config: NotifyConfig) -> NotifyController {
    NotifyService::new(notify_config).start(Some("NotifyService"))
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

fn build_tx_pool(
    tx_pool_config: TxPoolConfig,
    block_assembler_config: Option<BlockAssemblerConfig>,
    notify: NotifyController,
    txs_verify_cache: Arc<TokioRwLock<TxVerifyCache>>,
    snapshot: Arc<Snapshot>,
    snapshot_mgr: Arc<SnapshotMgr>,
    handle: &Handle,
) -> (TxPoolServiceBuilder, TxPoolController) {
    let (mut tx_pool_builder, tx_pool_controller) = TxPoolServiceBuilder::new(
        tx_pool_config,
        snapshot,
        block_assembler_config,
        txs_verify_cache,
        snapshot_mgr,
        handle,
    );

    let notify_pending = notify.clone();
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

    let notify_proposed = notify.clone();
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

    tx_pool_builder.register_committed(Box::new(move |tx_pool: &mut TxPool, entry: TxEntry| {
        tx_pool.update_statics_for_remove_tx(entry.size, entry.cycles);
    }));

    let notify_reject = notify.clone();
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
    (tx_pool_builder, tx_pool_controller)
}
