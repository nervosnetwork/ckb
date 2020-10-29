//! TODO(doc): @quake
use crate::{migrations, Snapshot, SnapshotMgr};
use arc_swap::Guard;
use ckb_app_config::{BlockAssemblerConfig, DBConfig, NotifyConfig, StoreConfig, TxPoolConfig};
use ckb_chain_spec::consensus::Consensus;
use ckb_chain_spec::SpecError;
use ckb_db::RocksDB;
use ckb_db_migration::{DefaultMigration, Migrations};
use ckb_error::{Error, InternalErrorKind};
use ckb_notify::{NotifyController, NotifyService};
use ckb_proposal_table::{ProposalTable, ProposalView};
use ckb_store::ChainDB;
use ckb_store::{ChainStore, COLUMNS};
use ckb_tx_pool::{TokioRwLock, TxPoolController, TxPoolServiceBuilder};
use ckb_types::{
    core::{EpochExt, HeaderView},
    packed::Byte32,
    U256,
};
use ckb_verification::cache::TxVerifyCache;
use std::collections::HashSet;
use std::sync::Arc;

/// TODO(doc): @quake
#[derive(Clone)]
pub struct Shared {
    pub(crate) store: ChainDB,
    pub(crate) tx_pool_controller: TxPoolController,
    pub(crate) notify_controller: NotifyController,
    pub(crate) txs_verify_cache: Arc<TokioRwLock<TxVerifyCache>>,
    pub(crate) consensus: Arc<Consensus>,
    pub(crate) snapshot_mgr: Arc<SnapshotMgr>,
}

impl Shared {
    /// TODO(doc): @quake
    pub fn init(
        store: ChainDB,
        consensus: Consensus,
        tx_pool_config: TxPoolConfig,
        notify_config: NotifyConfig,
        block_assembler_config: Option<BlockAssemblerConfig>,
    ) -> Result<(Self, ProposalTable), Error> {
        let (tip_header, epoch) = Self::init_store(&store, &consensus)?;
        let total_difficulty = store
            .get_block_ext(&tip_header.hash())
            .ok_or_else(|| InternalErrorKind::Database.reason("failed to get tip's block_ext"))?
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

        let tx_pool_controller = tx_pool_builder.start();

        let shared = Shared {
            store,
            consensus,
            txs_verify_cache,
            snapshot_mgr,
            tx_pool_controller,
            notify_controller,
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
                        .reason("genesis does not exist in database")
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
    consensus: Option<Consensus>,
    tx_pool_config: Option<TxPoolConfig>,
    store_config: Option<StoreConfig>,
    block_assembler_config: Option<BlockAssemblerConfig>,
    notify_config: Option<NotifyConfig>,
    migrations: Migrations,
}

impl Default for SharedBuilder {
    fn default() -> Self {
        SharedBuilder {
            db: RocksDB::open_tmp(COLUMNS),
            consensus: None,
            tx_pool_config: None,
            notify_config: None,
            store_config: None,
            block_assembler_config: None,
            migrations: Migrations::default(),
        }
    }
}

const INIT_DB_VERSION: &str = "20191127135521";

impl SharedBuilder {
    /// TODO(doc): @quake
    pub fn with_db_config(config: &DBConfig) -> Self {
        let db = RocksDB::open(config, COLUMNS);
        let mut migrations = Migrations::default();
        migrations.add_migration(Box::new(DefaultMigration::new(INIT_DB_VERSION)));
        migrations.add_migration(Box::new(migrations::ChangeMoleculeTableToStruct));
        migrations.add_migration(Box::new(migrations::CellMigration));

        SharedBuilder {
            db,
            consensus: None,
            tx_pool_config: None,
            notify_config: None,
            store_config: None,
            block_assembler_config: None,
            migrations,
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

    /// TODO(doc): @quake
    pub fn build(self) -> Result<(Shared, ProposalTable), Error> {
        let consensus = self.consensus.unwrap_or_else(Consensus::default);
        let tx_pool_config = self.tx_pool_config.unwrap_or_else(Default::default);
        let notify_config = self.notify_config.unwrap_or_else(Default::default);
        let store_config = self.store_config.unwrap_or_else(Default::default);
        let db = self.migrations.migrate(self.db)?;
        let store = ChainDB::new(db, store_config);

        Shared::init(
            store,
            consensus,
            tx_pool_config,
            notify_config,
            self.block_assembler_config,
        )
    }
}
