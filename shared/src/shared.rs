use crate::cachedb::CacheDB;
use crate::cell_set::CellSet;
use crate::chain_state::ChainState;
use crate::error::SharedError;
use crate::index::ChainIndex;
use crate::store::ChainKVStore;
use crate::tx_pool::{TxPool, TxPoolConfig};
use crate::tx_proposal_table::TxProposalTable;
use crate::{COLUMNS, COLUMN_BLOCK_HEADER};
use ckb_chain_spec::consensus::{Consensus, ProposalWindow};
use ckb_core::block::Block;
use ckb_core::cell::{CellProvider, CellStatus};
use ckb_core::extras::BlockExt;
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{Capacity, OutPoint, ProposalShortId, Transaction};
use ckb_core::uncle::UncleBlock;
use ckb_db::{DBConfig, KeyValueDB, MemoryKeyValueDB, RocksDB};
use ckb_traits::{BlockMedianTimeContext, ChainProvider};
use ckb_util::Mutex;
use failure::Error;
use fnv::FnvHashSet;
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::sync::Arc;

#[derive(Debug)]
pub struct Shared<CI> {
    store: Arc<CI>,
    chain_state: Arc<Mutex<ChainState<CI>>>,
    consensus: Arc<Consensus>,
}

// https://github.com/rust-lang/rust/issues/40754
impl<CI: ChainIndex> ::std::clone::Clone for Shared<CI> {
    fn clone(&self) -> Self {
        Shared {
            store: Arc::clone(&self.store),
            chain_state: Arc::clone(&self.chain_state),
            consensus: Arc::clone(&self.consensus),
        }
    }
}

impl<CI: ChainIndex> Shared<CI> {
    pub fn new(
        store: CI,
        consensus: Consensus,
        txs_verify_cache_size: usize,
        tx_pool_config: TxPoolConfig,
    ) -> Self {
        let store = Arc::new(store);
        let chain_state = {
            // check head in store or save the genesis block as head
            let header = {
                let genesis = consensus.genesis_block();
                match store.get_tip_header() {
                    Some(h) => h,
                    None => {
                        store.init(&genesis);
                        genesis.header().clone()
                    }
                }
            };

            let tip_number = header.number();
            let proposal_window = consensus.tx_proposal_window();
            let proposal_ids = Self::init_proposal_ids(&store, proposal_window, tip_number);

            let cell_set = Self::init_cell_set(&store, tip_number);

            let total_difficulty = store
                .get_block_ext(&header.hash())
                .expect("block_ext stored")
                .total_difficulty;

            Arc::new(Mutex::new(ChainState::new(
                &store,
                header,
                total_difficulty,
                cell_set,
                proposal_ids,
                TxPool::new(tx_pool_config),
                LruCache::new(txs_verify_cache_size),
            )))
        };

        Shared {
            store,
            chain_state,
            consensus: Arc::new(consensus),
        }
    }

    pub fn chain_state(&self) -> &Mutex<ChainState<CI>> {
        &self.chain_state
    }

    pub fn store(&self) -> &Arc<CI> {
        &self.store
    }

    pub fn init_proposal_ids(
        store: &CI,
        proposal_window: ProposalWindow,
        tip_number: u64,
    ) -> TxProposalTable {
        let mut proposal_ids = TxProposalTable::new(proposal_window);
        let proposal_start = tip_number.saturating_sub(proposal_window.start());
        let proposal_end = tip_number.saturating_sub(proposal_window.end());
        for bn in proposal_start..=proposal_end {
            if let Some(hash) = store.get_block_hash(bn) {
                let mut ids_set = FnvHashSet::default();
                if let Some(ids) = store.get_block_proposal_txs_ids(&hash) {
                    ids_set.extend(ids)
                }

                if let Some(us) = store.get_block_uncles(&hash) {
                    for u in us {
                        let ids = u.proposal_transactions;
                        ids_set.extend(ids);
                    }
                }
                proposal_ids.update_or_insert(bn, ids_set);
            }
        }
        proposal_ids.finalize(tip_number);
        proposal_ids
    }

    pub fn init_cell_set(store: &CI, number: u64) -> CellSet {
        let mut cell_set = CellSet::new();

        for n in 0..=number {
            let hash = store.get_block_hash(n).unwrap();
            for tx in store.get_block_body(&hash).unwrap() {
                let inputs = tx.input_pts();
                let output_len = tx.outputs().len();

                for o in inputs {
                    cell_set.mark_dead(&o);
                }

                cell_set.insert(&tx.hash(), output_len);
            }
        }

        cell_set
    }
}

impl<CI: ChainIndex> CellProvider for Shared<CI> {
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        self.cell_at(out_point, |op| self.chain_state.lock().is_dead(op))
    }

    fn cell_at<F: Fn(&OutPoint) -> Option<bool>>(
        &self,
        out_point: &OutPoint,
        is_dead: F,
    ) -> CellStatus {
        let index = out_point.index as usize;
        if let Some(f) = is_dead(out_point) {
            if f {
                CellStatus::Dead
            } else {
                let transaction = self
                    .store
                    .get_transaction(&out_point.hash)
                    .expect("transaction must exist");
                CellStatus::Live(transaction.outputs()[index].clone())
            }
        } else {
            CellStatus::Unknown
        }
    }
}

impl<CI: ChainIndex> ChainProvider for Shared<CI> {
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

    fn genesis_hash(&self) -> H256 {
        self.consensus.genesis_block().header().hash()
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

    // TODO: find a way to write test for this once we can build a mock on
    // ChainIndex
    fn calculate_transaction_fee(&self, transaction: &Transaction) -> Result<Capacity, Error> {
        let mut fee = 0;
        for input in transaction.inputs() {
            let previous_output = &input.previous_output;
            match self.get_transaction(&previous_output.hash) {
                Some(previous_transaction) => {
                    let index = previous_output.index as usize;
                    if let Some(output) = previous_transaction.outputs().get(index) {
                        fee += output.capacity;
                    } else {
                        Err(SharedError::InvalidInput)?;
                    }
                }
                None => Err(SharedError::InvalidInput)?,
            }
        }
        let spent_capacity: Capacity = transaction
            .outputs()
            .iter()
            .map(|output| output.capacity)
            .sum();
        if spent_capacity > fee {
            Err(SharedError::InvalidOutput)?;
        }
        fee -= spent_capacity;
        Ok(fee)
    }

    // T_interval = L / C_m
    // HR_m = HR_last/ (1 + o)
    // Diff= HR_m * T_interval / H = Diff_last * o_last / o
    #[allow(clippy::op_ref)]
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
            if &difficulty > &max_difficulty {
                return Some(max_difficulty);
            }

            if &difficulty < min_difficulty {
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

impl<CI: ChainIndex> BlockMedianTimeContext for Shared<CI> {
    fn block_count(&self) -> u32 {
        self.consensus.median_time_block_count() as u32
    }
    fn timestamp(&self, hash: &H256) -> Option<u64> {
        self.block_header(hash).map(|header| header.timestamp())
    }
    fn parent_hash(&self, hash: &H256) -> Option<H256> {
        self.block_header(hash)
            .map(|header| header.parent_hash().to_owned())
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
            &[(COLUMN_BLOCK_HEADER.unwrap(), 4096)],
        ));
        self
    }
}

pub const MIN_TXS_VERIFY_CACHE_SIZE: Option<usize> = Some(100);

impl<DB: 'static + KeyValueDB> SharedBuilder<DB> {
    pub fn consensus(mut self, value: Consensus) -> Self {
        self.consensus = Some(value);
        self
    }

    pub fn tx_pool_config(mut self, config: TxPoolConfig) -> Self {
        self.tx_pool_config = Some(config);
        self
    }

    pub fn txs_verify_cache_size(mut self, value: usize) -> Self {
        if let Some(c) = self.tx_pool_config.as_mut() {
            c.txs_verify_cache_size = value;
        };
        self
    }

    pub fn build(self) -> Shared<ChainKVStore<DB>> {
        let store = ChainKVStore::new(self.db.unwrap());
        let consensus = self.consensus.unwrap_or_else(Consensus::default);
        let tx_pool_config = self.tx_pool_config.unwrap_or_else(Default::default);
        Shared::new(
            store,
            consensus,
            tx_pool_config.txs_verify_cache_size,
            tx_pool_config,
        )
    }
}
