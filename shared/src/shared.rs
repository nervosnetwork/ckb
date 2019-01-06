use crate::block_median_time_context::BlockMedianTimeContext;
use crate::cachedb::CacheDB;
use crate::error::SharedError;
use crate::index::ChainIndex;
use crate::store::ChainKVStore;
use crate::txo_set::{TxoSet, TxoSetDiff};
use crate::{COLUMNS, COLUMN_BLOCK_HEADER};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::cell::{CellProvider, CellStatus};
use ckb_core::extras::BlockExt;
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{Capacity, OutPoint, ProposalShortId, Transaction};
use ckb_core::transaction_meta::TransactionMeta;
use ckb_core::uncle::UncleBlock;
use ckb_db::diskdb::RocksDB;
use ckb_db::kvdb::KeyValueDB;
use ckb_db::memorydb::MemoryKeyValueDB;
use ckb_util::RwLock;
use fnv::FnvHashSet;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::path::Path;
use std::sync::Arc;

#[derive(Default, Debug, PartialEq, Clone, Eq)]
pub struct ChainState {
    tip_header: Header,
    total_difficulty: U256,
    txo_set: TxoSet,
}

impl ChainState {
    pub fn new(tip_header: Header, total_difficulty: U256, txo_set: TxoSet) -> Self {
        ChainState {
            tip_header,
            total_difficulty,
            txo_set,
        }
    }

    pub fn tip_number(&self) -> BlockNumber {
        self.tip_header.number()
    }

    pub fn tip_hash(&self) -> H256 {
        self.tip_header.hash()
    }

    pub fn total_difficulty(&self) -> &U256 {
        &self.total_difficulty
    }

    pub fn tip_header(&self) -> &Header {
        &self.tip_header
    }

    pub fn txo_set(&self) -> &TxoSet {
        &self.txo_set
    }

    pub fn is_spent(&self, o: &OutPoint) -> Option<bool> {
        self.txo_set.is_spent(o)
    }

    pub fn update_header(&mut self, header: Header) {
        self.tip_header = header;
    }

    pub fn update_difficulty(&mut self, difficulty: U256) {
        self.total_difficulty = difficulty;
    }

    pub fn update_txo_set(&mut self, diff: TxoSetDiff) {
        self.txo_set.update(diff);
    }
}

pub struct Shared<CI> {
    store: Arc<CI>,
    chain_state: Arc<RwLock<ChainState>>,
    consensus: Consensus,
}

// https://github.com/rust-lang/rust/issues/40754
impl<CI: ChainIndex> ::std::clone::Clone for Shared<CI> {
    fn clone(&self) -> Self {
        Shared {
            store: Arc::clone(&self.store),
            chain_state: Arc::clone(&self.chain_state),
            consensus: self.consensus.clone(),
        }
    }
}

impl<CI: ChainIndex> Shared<CI> {
    pub fn new(store: CI, consensus: Consensus) -> Self {
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

            let txo_set = Self::init_txo_set(&store, header.hash());

            let total_difficulty = store
                .get_block_ext(&header.hash())
                .expect("block_ext stored")
                .total_difficulty;

            Arc::new(RwLock::new(ChainState::new(
                header,
                total_difficulty,
                txo_set,
            )))
        };

        Shared {
            store: Arc::new(store),
            chain_state,
            consensus,
        }
    }

    pub fn chain_state(&self) -> &RwLock<ChainState> {
        &self.chain_state
    }

    pub fn store(&self) -> &Arc<CI> {
        &self.store
    }

    pub fn init_txo_set(store: &CI, mut hash: H256) -> TxoSet {
        let mut txo_set = TxoSet::new();
        let mut blocks = Vec::new();
        while let Some(block) = store.get_block(&hash) {
            let number = block.header().number();
            hash = block.header().parent_hash().clone();
            blocks.push(block);

            if number == 0 {
                break;
            }
        }

        for b in blocks.iter().rev() {
            for tx in b.commit_transactions() {
                let inputs = tx.input_pts();
                let tx_hash = tx.hash();
                let output_len = tx.outputs().len();

                for o in inputs {
                    txo_set.mark_spent(&o);
                }

                txo_set.insert(tx_hash, output_len);
            }
        }

        txo_set
    }
}

impl<CI: ChainIndex> CellProvider for Shared<CI> {
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        self.cell_at(out_point, |op| self.chain_state.read().is_spent(op))
    }

    fn cell_at<F: Fn(&OutPoint) -> Option<bool>>(
        &self,
        out_point: &OutPoint,
        is_spent: F,
    ) -> CellStatus {
        let index = out_point.index as usize;
        if let Some(f) = is_spent(out_point) {
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

pub trait ChainProvider: Sync + Send {
    fn block_body(&self, hash: &H256) -> Option<Vec<Transaction>>;

    fn block_header(&self, hash: &H256) -> Option<Header>;

    fn block_proposal_txs_ids(&self, hash: &H256) -> Option<Vec<ProposalShortId>>;

    fn union_proposal_ids_n(&self, bn: BlockNumber, n: usize) -> Vec<Vec<ProposalShortId>>;

    fn uncles(&self, hash: &H256) -> Option<Vec<UncleBlock>>;

    fn block_hash(&self, number: BlockNumber) -> Option<H256>;

    fn block_ext(&self, hash: &H256) -> Option<BlockExt>;

    fn block_number(&self, hash: &H256) -> Option<BlockNumber>;

    fn block(&self, hash: &H256) -> Option<Block>;

    fn genesis_hash(&self) -> H256;

    fn get_transaction(&self, hash: &H256) -> Option<Transaction>;

    fn contain_transaction(&self, hash: &H256) -> bool;

    fn block_reward(&self, block_number: BlockNumber) -> Capacity;

    fn get_ancestor(&self, base: &H256, number: BlockNumber) -> Option<Header>;

    // Loops through all inputs and outputs of given transaction to calculate
    // fee that miner can obtain. Could result in error state when input
    // transaction is missing.
    fn calculate_transaction_fee(&self, transaction: &Transaction)
        -> Result<Capacity, SharedError>;

    fn calculate_difficulty(&self, last: &Header) -> Option<U256>;

    fn consensus(&self) -> &Consensus;
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

    /// Proposals in blocks from bn-n(exclusive) to bn(inclusive)
    fn union_proposal_ids_n(&self, bn: BlockNumber, n: usize) -> Vec<Vec<ProposalShortId>> {
        let m = if bn > n as u64 { n } else { bn as usize };
        let mut ret = Vec::new();

        if let Some(mut hash) = self.block_hash(bn) {
            for _ in 0..m {
                let mut ids_set = FnvHashSet::default();

                if let Some(ids) = self.block_proposal_txs_ids(&hash) {
                    ids_set.extend(ids)
                }

                if let Some(us) = self.uncles(&hash) {
                    for u in us {
                        let ids = u.proposal_transactions;
                        ids_set.extend(ids);
                    }
                }

                let ids_vec: Vec<ProposalShortId> = ids_set.into_iter().collect();
                ret.push(ids_vec);

                hash = self.block_header(&hash).unwrap().parent_hash().clone();
            }
        }

        ret
    }

    // TODO: find a way to write test for this once we can build a mock on
    // ChainIndex
    fn calculate_transaction_fee(
        &self,
        transaction: &Transaction,
    ) -> Result<Capacity, SharedError> {
        let mut fee = 0;
        for input in transaction.inputs() {
            let previous_output = &input.previous_output;
            match self.get_transaction(&previous_output.hash) {
                Some(previous_transaction) => {
                    let index = previous_output.index as usize;
                    if index < previous_transaction.outputs().len() {
                        fee += previous_transaction.outputs()[index].capacity;
                    } else {
                        return Err(SharedError::InvalidInput);
                    }
                }
                None => return Err(SharedError::InvalidInput),
            }
        }
        let spent_capacity: Capacity = transaction
            .outputs()
            .iter()
            .map(|output| output.capacity)
            .sum();
        if spent_capacity > fee {
            return Err(SharedError::InvalidOutput);
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
        &self.consensus
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

pub struct SharedBuilder<CI> {
    store: CI,
    consensus: Option<Consensus>,
}

impl<CI: ChainIndex> SharedBuilder<CI> {
    pub fn new_memory() -> SharedBuilder<ChainKVStore<MemoryKeyValueDB>> {
        let db = MemoryKeyValueDB::open(COLUMNS as usize);
        SharedBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_simple(db)
    }

    pub fn new_rocks<P: AsRef<Path>>(path: P) -> SharedBuilder<ChainKVStore<CacheDB<RocksDB>>> {
        let db = CacheDB::new(
            RocksDB::open(path, COLUMNS),
            &[(COLUMN_BLOCK_HEADER.unwrap(), 4096)],
        );
        SharedBuilder::<ChainKVStore<CacheDB<RocksDB>>>::new_simple(db)
    }

    pub fn new_simple<T: 'static + KeyValueDB>(db: T) -> SharedBuilder<ChainKVStore<T>> {
        let mut consensus = Consensus::default();
        consensus.initial_block_reward = 50;
        SharedBuilder {
            store: ChainKVStore::new(db),
            consensus: Some(consensus),
        }
    }

    pub fn consensus(mut self, value: Consensus) -> Self {
        self.consensus = Some(value);
        self
    }

    pub fn build(self) -> Shared<CI> {
        let consensus = self.consensus.unwrap_or_else(Consensus::default);
        Shared::new(self.store, consensus)
    }
}
