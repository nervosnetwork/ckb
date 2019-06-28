use crate::chain_state::ChainState;
use crate::error::SharedError;
use crate::tx_pool::TxPoolConfig;
use ckb_chain_spec::consensus::Consensus;
use ckb_core::extras::EpochExt;
use ckb_core::header::Header;
use ckb_core::script::Script;
use ckb_core::Capacity;
use ckb_core::Cycle;
use ckb_db::{DBConfig, KeyValueDB, MemoryKeyValueDB, RocksDB};
use ckb_reward_calculator::RewardCalculator;
use ckb_script::ScriptConfig;
use ckb_store::{ChainKVStore, ChainStore, StoreConfig, COLUMNS};
use ckb_traits::ChainProvider;
use ckb_util::{lock_or_panic, Mutex, MutexGuard};
use failure::Error as FailureError;
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use std::sync::Arc;

const TXS_VERIFY_CACHE_SIZE: usize = 10_000;

#[derive(Debug)]
pub struct Shared<CS> {
    store: Arc<CS>,
    chain_state: Arc<Mutex<ChainState<CS>>>,
    txs_verify_cache: Arc<Mutex<LruCache<H256, Cycle>>>,
    consensus: Arc<Consensus>,
    script_config: ScriptConfig,
}

// https://github.com/rust-lang/rust/issues/40754
impl<CS: ChainStore> ::std::clone::Clone for Shared<CS> {
    fn clone(&self) -> Self {
        Shared {
            store: Arc::clone(&self.store),
            chain_state: Arc::clone(&self.chain_state),
            consensus: Arc::clone(&self.consensus),
            script_config: self.script_config.clone(),
            txs_verify_cache: Arc::clone(&self.txs_verify_cache),
        }
    }
}

impl<CS: ChainStore> Shared<CS> {
    pub fn init(
        store: CS,
        consensus: Consensus,
        tx_pool_config: TxPoolConfig,
        script_config: ScriptConfig,
    ) -> Result<Self, SharedError> {
        let store = Arc::new(store);
        let consensus = Arc::new(consensus);
        let txs_verify_cache = Arc::new(Mutex::new(LruCache::new(TXS_VERIFY_CACHE_SIZE)));
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

    pub fn lock_chain_state(&self) -> MutexGuard<ChainState<CS>> {
        lock_or_panic(&self.chain_state)
    }

    pub fn lock_txs_verify_cache(&self) -> MutexGuard<LruCache<H256, Cycle>> {
        lock_or_panic(&self.txs_verify_cache)
    }
}

impl<CS: ChainStore> ChainProvider for Shared<CS> {
    type Store = CS;

    fn store(&self) -> &Arc<CS> {
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

    fn finalize_block_reward(&self, parent: &Header) -> Result<(Script, Capacity), FailureError> {
        RewardCalculator::new(self).block_reward(parent)
    }

    fn consensus(&self) -> &Consensus {
        &*self.consensus
    }
}

pub struct SharedBuilder<DB: KeyValueDB> {
    db: Option<DB>,
    consensus: Option<Consensus>,
    tx_pool_config: Option<TxPoolConfig>,
    script_config: Option<ScriptConfig>,
    store_config: Option<StoreConfig>,
}

impl<DB: KeyValueDB> Default for SharedBuilder<DB> {
    fn default() -> Self {
        SharedBuilder {
            db: None,
            consensus: None,
            tx_pool_config: None,
            script_config: None,
            store_config: None,
        }
    }
}

impl SharedBuilder<MemoryKeyValueDB> {
    pub fn new() -> Self {
        SharedBuilder {
            db: Some(MemoryKeyValueDB::open(COLUMNS as usize)),
            consensus: None,
            tx_pool_config: None,
            script_config: None,
            store_config: None,
        }
    }
}

impl SharedBuilder<RocksDB> {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn db(mut self, config: &DBConfig) -> Self {
        self.db = Some(RocksDB::open(config, COLUMNS));
        self
    }
}

pub const MIN_TXS_VERIFY_CACHE_SIZE: Option<usize> = Some(100);

impl<DB: KeyValueDB> SharedBuilder<DB> {
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

    pub fn build(self) -> Result<Shared<ChainKVStore<DB>>, SharedError> {
        let store = ChainKVStore::with_config(
            self.db.unwrap(),
            self.store_config.unwrap_or_else(Default::default),
        );
        let consensus = self.consensus.unwrap_or_else(Consensus::default);
        let tx_pool_config = self.tx_pool_config.unwrap_or_else(Default::default);
        let script_config = self.script_config.unwrap_or_else(Default::default);
        Shared::init(store, consensus, tx_pool_config, script_config)
    }
}
