use crate::error::SharedError;
use arc_swap::Guard;
use ckb_chain_spec::consensus::Consensus;
use ckb_db::{DBConfig, RocksDB};
use ckb_logger::info_target;
use ckb_proposal_table::{ProposalTable, ProposalView};
use ckb_reward_calculator::RewardCalculator;
use ckb_script::ScriptConfig;
use ckb_snapshot::{Snapshot, SnapshotMgr};
use ckb_store::ChainDB;
use ckb_store::{ChainStore, StoreConfig, COLUMNS};
use ckb_traits::ChainProvider;
use ckb_tx_pool::{
    BlockAssemblerConfig, PollLock, TxPoolConfig, TxPoolController, TxPoolServiceBuiler,
};
use ckb_types::{
    core::{BlockReward, Cycle, EpochExt, HeaderView, TransactionMeta},
    packed::{Byte32, Script},
    prelude::*,
    U256,
};
use failure::Error as FailureError;
use im::hashmap::HashMap as HamtMap;
use lru_cache::LruCache;
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Clone)]
pub struct Shared {
    pub(crate) store: Arc<ChainDB>,
    pub(crate) tx_pool_controller: TxPoolController,
    pub(crate) txs_verify_cache: PollLock<LruCache<Byte32, Cycle>>,
    pub(crate) consensus: Arc<Consensus>,
    pub(crate) script_config: ScriptConfig,
    pub(crate) snapshot_mgr: Arc<SnapshotMgr>,
}

impl Shared {
    pub fn init(
        store: ChainDB,
        consensus: Consensus,
        tx_pool_config: TxPoolConfig,
        script_config: ScriptConfig,
        block_assembler_config: Option<BlockAssemblerConfig>,
    ) -> Result<(Self, ProposalTable), SharedError> {
        let (tip_header, epoch) = Self::init_store(&store, &consensus)?;
        let total_difficulty = store
            .get_block_ext(&tip_header.hash())
            .ok_or_else(|| SharedError::InvalidData("failed to get block_ext".to_owned()))?
            .total_difficulty;
        let (proposal_table, proposal_view) = Self::init_proposal_table(&store, &consensus);
        let cell_set = Self::init_cell_set(&store)?;

        let store = Arc::new(store);
        let consensus = Arc::new(consensus);

        let txs_verify_cache = PollLock::new(LruCache::new(tx_pool_config.max_verify_cache_size));
        let snapshot = Arc::new(Snapshot::new(
            tip_header,
            total_difficulty,
            epoch,
            store.get_snapshot(),
            cell_set,
            proposal_view,
            Arc::clone(&consensus),
        ));
        let snapshot_mgr = Arc::new(SnapshotMgr::new(Arc::clone(&snapshot)));

        let tx_pool_builer = TxPoolServiceBuiler::new(
            tx_pool_config,
            Arc::clone(&snapshot),
            script_config.clone(),
            block_assembler_config,
            txs_verify_cache.clone(),
            Arc::clone(&snapshot_mgr),
        );

        let tx_pool_controller = tx_pool_builer.start();

        let shared = Shared {
            store,
            consensus,
            script_config,
            txs_verify_cache,
            snapshot_mgr,
            tx_pool_controller,
        };

        Ok((shared, proposal_table))
    }

    pub(crate) fn init_cell_set(
        store: &ChainDB,
    ) -> Result<HamtMap<Byte32, TransactionMeta>, SharedError> {
        let mut cell_set = HamtMap::new();
        let mut count = 0;
        info_target!(crate::LOG_TARGET_CHAIN, "Start: loading live cells ...");
        store
            .traverse_cell_set(|tx_hash, tx_meta| {
                count += 1;
                cell_set.insert(tx_hash, tx_meta.unpack());
                if count % 10_000 == 0 {
                    info_target!(
                        crate::LOG_TARGET_CHAIN,
                        "    loading {} transactions which include live cells ...",
                        count
                    );
                }
                Ok(())
            })
            .map_err(|e| SharedError::InvalidData(format!("failed to init cell set {:?}", e)))?;
        info_target!(
            crate::LOG_TARGET_CHAIN,
            "Done: total {} transactions.",
            count
        );

        Ok(cell_set)
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
    ) -> Result<(HeaderView, EpochExt), SharedError> {
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
                        Err(SharedError::InvalidData(format!(
                            "mismatch genesis hash, expect {} but {} in database",
                            expect_genesis_hash, genesis_hash
                        )))
                    }
                } else {
                    Err(SharedError::InvalidData(
                        "the genesis hash was not found".to_owned(),
                    ))
                }
            }
            None => store
                .init(&consensus)
                .map_err(|e| {
                    SharedError::InvalidData(format!("failed to init genesis block {:?}", e))
                })
                .map(|_| {
                    (
                        consensus.genesis_block().header().to_owned(),
                        consensus.genesis_epoch_ext().to_owned(),
                    )
                }),
        }
    }

    pub fn tx_pool_controller(&self) -> &TxPoolController {
        &self.tx_pool_controller
    }

    pub fn txs_verify_cache(&self) -> PollLock<LruCache<Byte32, Cycle>> {
        self.txs_verify_cache.clone()
    }

    pub fn snapshot(&self) -> Guard<Arc<Snapshot>> {
        self.snapshot_mgr.load()
    }

    pub fn store_snapshot(&self, snapshot: Arc<Snapshot>) {
        self.snapshot_mgr.store(snapshot)
    }

    pub fn new_snapshot(
        &self,
        tip_header: HeaderView,
        total_difficulty: U256,
        epoch_ext: EpochExt,
        cell_set: HamtMap<Byte32, TransactionMeta>,
        proposals: ProposalView,
    ) -> Arc<Snapshot> {
        Arc::new(Snapshot::new(
            tip_header,
            total_difficulty,
            epoch_ext,
            self.store.get_snapshot(),
            cell_set,
            proposals,
            Arc::clone(&self.consensus),
        ))
    }
}

impl ChainProvider for Shared {
    type Store = ChainDB;

    fn store(&self) -> &Self::Store {
        &self.store
    }

    fn script_config(&self) -> &ScriptConfig {
        &self.script_config
    }

    fn genesis_hash(&self) -> Byte32 {
        self.consensus.genesis_hash()
    }

    fn get_block_epoch(&self, hash: &Byte32) -> Option<EpochExt> {
        self.store()
            .get_block_epoch_index(hash)
            .and_then(|index| self.store().get_epoch_ext(&index))
    }

    fn next_epoch_ext(&self, last_epoch: &EpochExt, header: &HeaderView) -> Option<EpochExt> {
        self.consensus.next_epoch_ext(
            last_epoch,
            header,
            |hash| self.store.get_block_header(hash),
            |hash| {
                self.store
                    .get_block_ext(hash)
                    .map(|ext| ext.total_uncles_count)
            },
        )
    }

    fn finalize_block_reward(
        &self,
        parent: &HeaderView,
    ) -> Result<(Script, BlockReward), FailureError> {
        RewardCalculator::new(self.consensus(), self.store()).block_reward(parent)
    }

    fn consensus(&self) -> &Consensus {
        &*self.consensus
    }
}

pub struct SharedBuilder {
    db: RocksDB,
    consensus: Option<Consensus>,
    tx_pool_config: Option<TxPoolConfig>,
    script_config: Option<ScriptConfig>,
    store_config: Option<StoreConfig>,
    block_assembler_config: Option<BlockAssemblerConfig>,
}

impl Default for SharedBuilder {
    fn default() -> Self {
        SharedBuilder {
            db: RocksDB::open_tmp(COLUMNS),
            consensus: None,
            tx_pool_config: None,
            script_config: None,
            store_config: None,
            block_assembler_config: None,
        }
    }
}

impl SharedBuilder {
    pub fn with_db_config(config: &DBConfig) -> Self {
        let db = RocksDB::open(config, COLUMNS);
        SharedBuilder {
            db,
            ..Default::default()
        }
    }
}

impl SharedBuilder {
    pub fn consensus(mut self, value: Consensus) -> Self {
        self.consensus = Some(value);
        self
    }

    pub fn tx_pool_config(mut self, config: TxPoolConfig) -> Self {
        self.tx_pool_config = Some(config);
        self
    }

    pub fn script_config(mut self, config: ScriptConfig) -> Self {
        self.script_config = Some(config);
        self
    }

    pub fn store_config(mut self, config: StoreConfig) -> Self {
        self.store_config = Some(config);
        self
    }

    pub fn block_assembler_config(mut self, config: Option<BlockAssemblerConfig>) -> Self {
        self.block_assembler_config = config;
        self
    }

    pub fn build(self) -> Result<(Shared, ProposalTable), SharedError> {
        if let Some(config) = self.store_config {
            config.apply()
        }
        let consensus = self.consensus.unwrap_or_else(Consensus::default);
        let tx_pool_config = self.tx_pool_config.unwrap_or_else(Default::default);
        let script_config = self.script_config.unwrap_or_else(Default::default);
        let store = ChainDB::new(self.db);
        Shared::init(
            store,
            consensus,
            tx_pool_config,
            script_config,
            self.block_assembler_config,
        )
    }
}
