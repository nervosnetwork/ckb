use crate::chain_state::ChainState;
use crate::store::ChainKVStore;
use crate::store::ChainStore;
use crate::tx_pool::TxPoolConfig;
use crate::{COLUMNS, COLUMN_BLOCK_HEADER};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::extras::BlockExt;
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{Capacity, ProposalShortId, Transaction};
use ckb_core::uncle::UncleBlock;
use ckb_db::{CacheDB, DBConfig, KeyValueDB, MemoryKeyValueDB, RocksDB};
use ckb_traits::ChainProvider;
use ckb_util::Mutex;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
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
    pub fn new(store: CS, consensus: Consensus, tx_pool_config: TxPoolConfig) -> Self {
        let store = Arc::new(store);
        let consensus = Arc::new(consensus);
        let chain_state = Arc::new(Mutex::new(ChainState::new(
            &store,
            Arc::clone(&consensus),
            tx_pool_config,
        )));

        Shared {
            store,
            chain_state,
            consensus,
        }
    }

    pub fn chain_state(&self) -> &Mutex<ChainState<CS>> {
        &self.chain_state
    }

    pub fn store(&self) -> &Arc<CS> {
        &self.store
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

    fn get_transaction(&self, hash: &H256) -> Option<Transaction> {
        self.store.get_transaction(hash)
    }

    fn contain_transaction(&self, hash: &H256) -> bool {
        self.store.get_transaction_address(hash).is_some()
    }

    fn block_reward(&self, _block_number: BlockNumber) -> Capacity {
        // TODO: block reward calculation algorithm
        self.consensus.initial_block_reward()
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

    // T_interval = L / C_m
    // HR_m = HR_last/ (1 + o)
    // Diff= HR_m * T_interval / H = Diff_last * o_last / o
    fn calculate_difficulty(&self, last: &Header) -> Option<U256> {
        let last_hash = last.hash();
        let last_number = last.number();
        let last_difficulty = last.difficulty();

        let interval = self.consensus.difficulty_adjustment_interval();

        if (last_number + 1) % interval != 0 {
            return Some(last_difficulty.clone());
        }

        let start = last_number.saturating_sub(interval);
        if let Some(start_header) = self.get_ancestor(&last_hash, start) {
            let start_total_uncles_count = self
                .block_ext(&start_header.hash())
                .expect("block_ext exist")
                .total_uncles_count;

            let last_total_uncles_count = self
                .block_ext(&last_hash)
                .expect("block_ext exist")
                .total_uncles_count;

            let difficulty = last_difficulty
                * U256::from(last_total_uncles_count - start_total_uncles_count)
                * U256::from((1.0 / self.consensus.orphan_rate_target()) as u64)
                / U256::from(interval);

            let min_difficulty = self.consensus.min_difficulty();
            let max_difficulty = last_difficulty * 2u32;
            if difficulty > max_difficulty {
                return Some(max_difficulty);
            }

            if difficulty.lt(min_difficulty) {
                return Some(min_difficulty.clone());
            }
            return Some(difficulty);
        }
        None
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

    pub fn build(self) -> Shared<ChainKVStore<DB>> {
        let store = ChainKVStore::new(self.db.unwrap());
        let consensus = self.consensus.unwrap_or_else(Consensus::default);
        let tx_pool_config = self.tx_pool_config.unwrap_or_else(Default::default);
        Shared::new(store, consensus, tx_pool_config)
    }
}
