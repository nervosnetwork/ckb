use crate::chain_state::ChainState;
use crate::error::SharedError;
use crate::tx_pool::TxPoolConfig;
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::extras::{BlockExt, EpochExt};
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{Capacity, ProposalShortId, Transaction};
use ckb_core::uncle::UncleBlock;
use ckb_db::{CacheDB, DBConfig, KeyValueDB, MemoryKeyValueDB, RocksDB};
use ckb_store::{ChainKVStore, ChainStore, COLUMNS, COLUMN_BLOCK_HEADER};
use ckb_traits::ChainProvider;
use ckb_util::Mutex;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::cmp;
use std::sync::Arc;

#[derive(Debug)]
pub struct Shared<CS> {
    store: Arc<CS>,
    chain_state: Arc<Mutex<ChainState<CS>>>,
    consensus: Arc<Consensus>,
}

// https://github.com/rust-lang/rust/issues/40754
impl<CS: ChainStore> ::std::clone::Clone for Shared<CS> {
    fn clone(&self) -> Self {
        Shared {
            store: Arc::clone(&self.store),
            chain_state: Arc::clone(&self.chain_state),
            consensus: Arc::clone(&self.consensus),
        }
    }
}

impl<CS: ChainStore> Shared<CS> {
    pub fn init(
        store: CS,
        consensus: Consensus,
        tx_pool_config: TxPoolConfig,
    ) -> Result<Self, SharedError> {
        let store = Arc::new(store);
        let consensus = Arc::new(consensus);
        let chain_state = Arc::new(Mutex::new(ChainState::init(
            &store,
            Arc::clone(&consensus),
            tx_pool_config,
        )?));

        Ok(Shared {
            store,
            chain_state,
            consensus,
        })
    }

    pub fn chain_state(&self) -> &Mutex<ChainState<CS>> {
        &self.chain_state
    }

    pub fn store(&self) -> &Arc<CS> {
        &self.store
    }

    fn fix_epoch_length(&self, raw: BlockNumber) -> BlockNumber {
        let max_length = self.consensus.max_epoch_length();
        let min_length = self.consensus.min_epoch_length();
        cmp::max(cmp::min(max_length, raw), min_length)
    }

    fn fix_epoch_difficulty(&self, last: U256, raw: U256) -> U256 {
        let min_difficulty = cmp::max(self.consensus.min_difficulty().clone(), &last / 2u64);
        let max_difficulty = last * 2u32;

        if raw > max_difficulty {
            return max_difficulty;
        }

        if raw < min_difficulty {
            return min_difficulty.clone();
        }
        raw
    }
}

impl<CS: ChainStore> ChainProvider for Shared<CS> {
    fn block(&self, hash: &H256) -> Option<Block> {
        self.store.get_block(hash)
    }

    fn block_body(&self, hash: &H256) -> Option<Vec<Transaction>> {
        self.store.get_block_body(hash)
    }

    fn block_header(&self, hash: &H256) -> Option<Header> {
        self.store.get_header(hash)
    }

    fn block_proposal_txs_ids(&self, hash: &H256) -> Option<Vec<ProposalShortId>> {
        self.store.get_block_proposal_txs_ids(hash)
    }

    fn uncles(&self, hash: &H256) -> Option<Vec<UncleBlock>> {
        self.store.get_block_uncles(hash)
    }

    fn block_hash(&self, number: BlockNumber) -> Option<H256> {
        self.store.get_block_hash(number)
    }

    fn block_ext(&self, hash: &H256) -> Option<BlockExt> {
        self.store.get_block_ext(hash)
    }

    fn block_number(&self, hash: &H256) -> Option<BlockNumber> {
        self.store.get_block_number(hash)
    }

    fn genesis_hash(&self) -> &H256 {
        self.consensus.genesis_hash()
    }

    fn get_transaction(&self, hash: &H256) -> Option<(Transaction, H256)> {
        self.store.get_transaction(hash)
    }

    fn contain_transaction(&self, hash: &H256) -> bool {
        self.store.get_transaction_address(hash).is_some()
    }

    fn get_ancestor(&self, base: &H256, number: BlockNumber) -> Option<Header> {
        // if base in the main chain
        if let Some(n_number) = self.block_number(base) {
            if number > n_number {
                return None;
            } else {
                return self
                    .block_hash(number)
                    .and_then(|hash| self.block_header(&hash));
            }
        }

        // if base in the fork
        if let Some(header) = self.block_header(base) {
            let mut n_number = header.number();
            let mut index_walk = header;
            if number > n_number {
                return None;
            }

            while n_number > number {
                if let Some(header) = self.block_header(&index_walk.parent_hash()) {
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

    fn is_epoch_end(&self, epoch: &EpochExt, number: BlockNumber) -> bool {
        (epoch.start() + epoch.length() - 1) == number
    }

    fn next_epoch_ext(&self, last_epoch: &EpochExt, header: &Header) -> Option<EpochExt> {
        let start = last_epoch.start();
        let last_epoch_length = last_epoch.length();

        if !self.is_epoch_end(last_epoch, header.number()) {
            return None;
        }

        let last_hash = header.hash();
        let last_difficulty = header.difficulty();
        let target_recip = self.consensus.orphan_rate_target_recip();
        let epoch_duration = self.consensus.epoch_duration();

        if let Some(start_header) = self.get_ancestor(&last_hash, start) {
            let start_total_uncles_count = self
                .block_ext(&start_header.hash())
                .expect("block_ext exist")
                .total_uncles_count;

            let last_total_uncles_count = self
                .block_ext(&last_hash)
                .expect("block_ext exist")
                .total_uncles_count;

            let last_uncles_count = last_total_uncles_count - start_total_uncles_count;

            let epoch_ext = if last_uncles_count > 0 {
                let last_duration = header.timestamp().saturating_sub(start_header.timestamp());
                if last_duration == 0 {
                    return None;
                }

                let numerator =
                    (last_uncles_count + last_epoch_length) * epoch_duration * last_epoch_length;
                let denominator = (target_recip + 1) * last_uncles_count * last_duration;
                let raw_next_epoch_length = numerator / denominator;
                let next_epoch_length = self.fix_epoch_length(raw_next_epoch_length);

                let raw_difficulty =
                    last_difficulty * U256::from(last_uncles_count) * U256::from(target_recip)
                        / U256::from(last_epoch_length);

                let difficulty = self.fix_epoch_difficulty(last_difficulty.clone(), raw_difficulty);

                let block_reward =
                    Capacity::shannons(self.consensus.epoch_reward().as_u64() / next_epoch_length);
                let remainder_reward =
                    Capacity::shannons(self.consensus.epoch_reward().as_u64() / next_epoch_length);

                EpochExt::new(
                    last_epoch.number() + 1, // number
                    block_reward,
                    remainder_reward,        // remainder_reward
                    header.number() + 1,     // start
                    next_epoch_length,       // length
                    difficulty               // difficulty,
                )
            } else {
                let next_epoch_length = self.consensus.max_epoch_length();
                let difficulty = cmp::max(
                    self.consensus.min_difficulty().clone(),
                    last_difficulty / 2u64,
                );

                let block_reward =
                    Capacity::shannons(self.consensus.epoch_reward().as_u64() / next_epoch_length);
                let remainder_reward =
                    Capacity::shannons(self.consensus.epoch_reward().as_u64() / next_epoch_length);
                EpochExt::new(
                    last_epoch.number() + 1, // number
                    block_reward,
                    remainder_reward,        // remainder_reward
                    header.number() + 1,     // start
                    next_epoch_length,       // length
                    difficulty               // difficulty,
                )
            };

            Some(epoch_ext)
        } else {
            None
        }
    }

    fn consensus(&self) -> &Consensus {
        &*self.consensus
    }
}

pub struct SharedBuilder<DB: KeyValueDB> {
    db: Option<DB>,
    consensus: Option<Consensus>,
    tx_pool_config: Option<TxPoolConfig>,
}

impl<DB: KeyValueDB> Default for SharedBuilder<DB> {
    fn default() -> Self {
        SharedBuilder {
            db: None,
            consensus: None,
            tx_pool_config: None,
        }
    }
}

impl SharedBuilder<MemoryKeyValueDB> {
    pub fn new() -> Self {
        SharedBuilder {
            db: Some(MemoryKeyValueDB::open(COLUMNS as usize)),
            consensus: None,
            tx_pool_config: None,
        }
    }
}

impl SharedBuilder<CacheDB<RocksDB>> {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn db(mut self, config: &DBConfig) -> Self {
        self.db = Some(CacheDB::new(
            RocksDB::open(config, COLUMNS),
            &[(COLUMN_BLOCK_HEADER, 4096)],
        ));
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

    pub fn build(self) -> Result<Shared<ChainKVStore<DB>>, SharedError> {
        let store = ChainKVStore::new(self.db.unwrap());
        let consensus = self.consensus.unwrap_or_else(Consensus::default);
        let tx_pool_config = self.tx_pool_config.unwrap_or_else(Default::default);
        Shared::init(store, consensus, tx_pool_config)
    }
}
