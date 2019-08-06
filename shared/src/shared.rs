use crate::chain_state::ChainState;
use crate::tx_pool::TxPoolConfig;
use ckb_chain_spec::consensus::Consensus;
use ckb_core::extras::EpochExt;
use ckb_core::header::Header;
use ckb_core::reward::BlockReward;
use ckb_core::script::Script;
use ckb_core::Cycle;
use ckb_db::{DBConfig, RocksDB};
use ckb_error::Error;
use ckb_reward_calculator::RewardCalculator;
use ckb_script::ScriptConfig;
use ckb_store::ChainDB;
use ckb_store::{ChainStore, StoreConfig, COLUMNS};
use ckb_traits::ChainProvider;
use ckb_util::{lock_or_panic, Mutex, MutexGuard};
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use std::sync::Arc;

#[derive(Clone)]
pub struct Shared {
    store: Arc<ChainDB>,
    chain_state: Arc<Mutex<ChainState>>,
    txs_verify_cache: Arc<Mutex<LruCache<H256, Cycle>>>,
    consensus: Arc<Consensus>,
    script_config: ScriptConfig,
}

impl Shared {
    pub fn init(
        store: ChainDB,
        consensus: Consensus,
        tx_pool_config: TxPoolConfig,
        script_config: ScriptConfig,
    ) -> Result<Self, Error> {
        let store = Arc::new(store);
        let consensus = Arc::new(consensus);
        let txs_verify_cache = Arc::new(Mutex::new(LruCache::new(
            tx_pool_config.max_verify_cache_size,
        )));
        let chain_state = Arc::new(Mutex::new(ChainState::init(
            &store,
            Arc::clone(&consensus),
            tx_pool_config,
            script_config.clone(),
        )?));

        Ok(Shared {
            store,
            chain_state,
            consensus,
            script_config,
            txs_verify_cache,
        })
    }

    pub fn lock_chain_state(&self) -> MutexGuard<ChainState> {
        lock_or_panic(&self.chain_state)
    }

    pub fn lock_txs_verify_cache(&self) -> MutexGuard<LruCache<H256, Cycle>> {
        lock_or_panic(&self.txs_verify_cache)
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

    fn genesis_hash(&self) -> &H256 {
        self.consensus.genesis_hash()
    }

    fn get_block_epoch(&self, hash: &H256) -> Option<EpochExt> {
        self.store()
            .get_block_epoch_index(hash)
            .and_then(|index| self.store().get_epoch_ext(&index))
    }

    fn next_epoch_ext(&self, last_epoch: &EpochExt, header: &Header) -> Option<EpochExt> {
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

    fn finalize_block_reward(&self, parent: &Header) -> Result<(Script, BlockReward), Error> {
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
}

impl Default for SharedBuilder {
    fn default() -> Self {
        SharedBuilder {
            db: RocksDB::open_tmp(COLUMNS),
            consensus: None,
            tx_pool_config: None,
            script_config: None,
            store_config: None,
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

    pub fn build(self) -> Result<Shared, Error> {
        if let Some(config) = self.store_config {
            config.apply()
        }
        let consensus = self.consensus.unwrap_or_else(Consensus::default);
        let tx_pool_config = self.tx_pool_config.unwrap_or_else(Default::default);
        let script_config = self.script_config.unwrap_or_else(Default::default);
        let store = ChainDB::new(self.db);
        Shared::init(store, consensus, tx_pool_config, script_config)
    }
}
