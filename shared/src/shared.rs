use crate::chain_state::ChainState;
use crate::error::SharedError;
use crate::tx_pool::TxPoolConfig;
use ckb_chain_spec::consensus::Consensus;
use ckb_core::extras::EpochExt;
use ckb_core::header::{BlockNumber, Header};
use ckb_core::script::Script;
use ckb_core::transaction::{ProposalShortId, Transaction};
use ckb_core::Capacity;
use ckb_core::Cycle;
use ckb_db::{DBConfig, KeyValueDB, MemoryKeyValueDB, RocksDB};
use ckb_script::ScriptConfig;
use ckb_store::{ChainKVStore, ChainStore, StoreConfig, COLUMNS};
use ckb_traits::ChainProvider;
use ckb_util::FnvHashSet;
use ckb_util::{lock_or_panic, Mutex, MutexGuard};
use failure::Error as FailureError;
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use std::cmp;
use std::collections::BTreeMap;
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

    fn get_proposal_ids_by_hash(&self, hash: &H256) -> FnvHashSet<ProposalShortId> {
        let store = self.store();
        let mut ids_set = FnvHashSet::default();
        if let Some(ids) = store.get_block_proposal_txs_ids(&hash) {
            ids_set.extend(ids)
        }
        if let Some(us) = store.get_block_uncles(&hash) {
            for u in us {
                ids_set.extend(u.proposals);
            }
        }
        ids_set
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

    fn get_ancestor(&self, base: &H256, number: BlockNumber) -> Option<Header> {
        // if base in the main chain
        if let Some(n_number) = self.store.get_block_number(base) {
            if number > n_number {
                return None;
            } else {
                return self
                    .store
                    .get_block_hash(number)
                    .and_then(|hash| self.store.get_block_header(&hash));
            }
        }

        // if base in the fork
        if let Some(header) = self.store.get_block_header(base) {
            let mut n_number = header.number();
            let mut index_walk = header;
            if number > n_number {
                return None;
            }

            while n_number > number {
                if let Some(header) = self.store.get_block_header(&index_walk.parent_hash()) {
                    index_walk = header;
                    n_number -= 1;
                } else {
                    return None;
                }
            }
            return Some(index_walk);
        }
        None
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
        let current_number = parent.number() + 1;
        let proposal_window = self.consensus().tx_proposal_window();
        let target_number = self
            .consensus()
            .finalize_target(current_number)
            .ok_or_else(|| SharedError::FinalizeTarget(current_number))?;

        let store = self.store();

        let target = self
            .get_ancestor(parent.hash(), target_number)
            .ok_or_else(|| SharedError::FinalizeTarget(current_number))?;

        let mut target_proposals = self.get_proposal_ids_by_hash(target.hash());

        let target_ext = store
            .get_block_ext(target.hash())
            .expect("block body stored");

        let target_lock = Script::from_witness(
            &store
                .get_cellbase(target.hash())
                .expect("target cellbase exist")
                .witnesses()[0],
        )
        .expect("cellbase checked");

        let mut reward = Capacity::zero();
        // tx commit start at least number 2
        let commit_start = cmp::max(current_number.saturating_sub(proposal_window.length()), 2);
        let proposal_start = cmp::max(commit_start.saturating_sub(proposal_window.start()), 1);

        let mut proposal_table = BTreeMap::new();
        for bn in proposal_start..target_number {
            let proposals = store
                .get_block_hash(bn)
                .map(|hash| self.get_proposal_ids_by_hash(&hash))
                .expect("finalize target exist");
            proposal_table.insert(bn, proposals);
        }

        let mut index = parent.to_owned();
        for (id, tx_fee) in store
            .get_block_body(index.hash())
            .expect("block body stored")
            .iter()
            .skip(1)
            .map(Transaction::proposal_short_id)
            .zip(
                store
                    .get_block_ext(index.hash())
                    .expect("block body stored")
                    .txs_fees
                    .iter(),
            )
        {
            if target_proposals.remove(&id) {
                reward = reward.safe_add(tx_fee.safe_mul_ratio(4, 10)?)?;
            }
        }

        index = store
            .get_block_header(index.parent_hash())
            .expect("header stored");

        while index.number() >= commit_start {
            let proposal_start =
                cmp::max(index.number().saturating_sub(proposal_window.start()), 1);
            let previous_ids: FnvHashSet<ProposalShortId> = proposal_table
                .range(proposal_start..)
                .flat_map(|(_, ids)| ids.iter().cloned())
                .collect();
            for (id, tx_fee) in store
                .get_block_body(index.hash())
                .expect("block body stored")
                .iter()
                .skip(1)
                .map(Transaction::proposal_short_id)
                .zip(
                    store
                        .get_block_ext(index.hash())
                        .expect("block body stored")
                        .txs_fees
                        .iter(),
                )
            {
                if target_proposals.remove(&id) && !previous_ids.contains(&id) {
                    reward = reward.safe_add(tx_fee.safe_mul_ratio(4, 10)?)?;
                }
            }

            index = store
                .get_block_header(index.parent_hash())
                .expect("header stored");
        }

        let txs_fees: Capacity =
            target_ext
                .txs_fees
                .iter()
                .try_fold(Capacity::zero(), |acc, tx_fee| {
                    tx_fee.safe_mul_ratio(4, 10).and_then(|proposer| {
                        tx_fee
                            .safe_sub(proposer)
                            .and_then(|miner| acc.safe_add(miner))
                    })
                })?;
        reward = reward.safe_add(txs_fees)?;

        let target_parent_hash = target.parent_hash();
        let target_parent_epoch = self
            .get_block_epoch(target_parent_hash)
            .expect("target parent exist");
        let target_parent = self
            .store
            .get_block_header(target_parent_hash)
            .expect("target parent exist");
        let epoch = self
            .next_epoch_ext(&target_parent_epoch, &target_parent)
            .unwrap_or(target_parent_epoch);

        let block_reward = epoch
            .block_reward(target.number())
            .expect("target block reward");

        reward = reward.safe_add(block_reward)?;

        Ok((target_lock, reward))
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
