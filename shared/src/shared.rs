use super::{COLUMNS, COLUMN_BLOCK_HEADER};
use bigint::{H256, U256};
use cachedb::CacheDB;
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
use error::SharedError;
use fnv::FnvHashSet;
use index::ChainIndex;
use std::path::Path;
use std::sync::Arc;
use store::ChainKVStore;

#[derive(Default, Debug, PartialEq, Clone, Eq)]
pub struct TipHeader {
    inner: Header,
    total_difficulty: U256,
    output_root: H256,
}

impl TipHeader {
    pub fn new(header: Header, total_difficulty: U256, output_root: H256) -> TipHeader {
        TipHeader {
            inner: header,
            total_difficulty,
            output_root,
        }
    }

    pub fn number(&self) -> BlockNumber {
        self.inner.number()
    }

    pub fn hash(&self) -> H256 {
        self.inner.hash()
    }

    pub fn total_difficulty(&self) -> U256 {
        self.total_difficulty
    }

    pub fn inner(&self) -> &Header {
        &self.inner
    }

    pub fn into_inner(self) -> Header {
        self.inner
    }

    pub fn output_root(&self) -> H256 {
        self.output_root
    }
}

pub struct Shared<CI> {
    store: Arc<CI>,
    tip_header: Arc<RwLock<TipHeader>>,
    consensus: Consensus,
}

impl<CI: ChainIndex> ::std::clone::Clone for Shared<CI> {
    fn clone(&self) -> Self {
        Shared {
            store: Arc::clone(&self.store),
            tip_header: Arc::clone(&self.tip_header),
            consensus: self.consensus.clone(),
        }
    }
}

impl<CI: ChainIndex> Shared<CI> {
    pub fn new(store: CI, consensus: Consensus) -> Self {
        let tip_header = {
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

            let output_root = match store.get_output_root(&header.hash()) {
                Some(h) => h,
                None => H256::zero(),
            };

            let total_difficulty = store
                .get_block_ext(&header.hash())
                .expect("block_ext stored")
                .total_difficulty;

            Arc::new(RwLock::new(TipHeader::new(
                header,
                total_difficulty,
                output_root,
            )))
        };

        Shared {
            store: Arc::new(store),
            tip_header,
            consensus,
        }
    }

    pub fn tip_header(&self) -> &RwLock<TipHeader> {
        &self.tip_header
    }

    pub fn store(&self) -> &Arc<CI> {
        &self.store
    }

    pub fn get_tip_uncles(&self) -> Vec<UncleBlock> {
        let max_uncles_age = self.consensus().max_uncles_age();
        let tip_header = self.tip_header().read();
        let header = tip_header.inner();
        let mut excluded = FnvHashSet::default();

        // cB
        // tip      1 depth, valid uncle
        // tip.p^0  ---/  2
        // tip.p^1  -----/  3
        // tip.p^2  -------/  4
        // tip.p^3  ---------/  5
        // tip.p^4  -----------/  6
        // tip.p^5  -------------/
        // tip.p^6
        let mut block_hash = header.hash();
        excluded.insert(block_hash);
        for _depth in 0..max_uncles_age {
            if let Some(block) = self.block(&block_hash) {
                excluded.insert(block.header().parent_hash());
                for uncle in block.uncles() {
                    excluded.insert(uncle.header.hash());
                }

                block_hash = block.header().parent_hash();
            } else {
                break;
            }
        }

        let tip_difficulty_epoch =
            header.number() / self.consensus().difficulty_adjustment_interval();

        let max_uncles_len = self.consensus().max_uncles_len();
        let mut included = FnvHashSet::default();
        let mut uncles = Vec::with_capacity(max_uncles_len);
        let current_number = tip_header.number() + 1;

        for hash in self.store().get_candidate_uncles() {
            if uncles.len() == max_uncles_len {
                break;
            }

            if let Some(uncle_header) = self.store().get_header(&hash) {
                let block_difficulty_epoch =
                    uncle_header.number() / self.consensus().difficulty_adjustment_interval();
                let depth = current_number.saturating_sub(uncle_header.number());

                // uncle must be same difficulty epoch with tip
                if uncle_header.difficulty() == header.difficulty()
                    && block_difficulty_epoch == tip_difficulty_epoch
                    && depth < max_uncles_age as u64
                    && depth >= 1
                    && !included.contains(&hash)
                    && !excluded.contains(&hash)
                {
                    if let Some(cellbase) =
                        self.store().get_transaction(&uncle_header.cellbase_id())
                    {
                        let uncle = UncleBlock {
                            header: uncle_header,
                            cellbase,
                            proposal_transactions: self
                                .store()
                                .get_block_proposal_txs_ids(&hash)
                                .unwrap_or_else(Vec::new),
                        };
                        uncles.push(uncle);
                        included.insert(hash);
                    }
                }
            }
        }

        uncles
    }
}

impl<CI: ChainIndex> CellProvider for Shared<CI> {
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        let index = out_point.index as usize;
        let tip_header = self.tip_header().read();
        if let Some(meta) = self.get_transaction_meta(&tip_header.output_root, &out_point.hash) {
            if index < meta.len() {
                if !meta.is_spent(index) {
                    let mut transaction = self
                        .store
                        .get_transaction(&out_point.hash)
                        .expect("transaction must exist");
                    CellStatus::Current(transaction.outputs()[index].clone())
                } else {
                    CellStatus::Old
                }
            } else {
                CellStatus::Unknown
            }
        } else {
            CellStatus::Unknown
        }
    }

    fn cell_at(&self, out_point: &OutPoint, parent: &H256) -> CellStatus {
        let index = out_point.index as usize;
        if let Some(meta) = self.get_transaction_meta_at(&out_point.hash, parent) {
            if index < meta.len() {
                if !meta.is_spent(index) {
                    let mut transaction = self
                        .store
                        .get_transaction(&out_point.hash)
                        .expect("transaction must exist");
                    CellStatus::Current(transaction.outputs()[index].clone())
                } else {
                    CellStatus::Old
                }
            } else {
                CellStatus::Unknown
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

    fn output_root(&self, hash: &H256) -> Option<H256>;

    fn block_number(&self, hash: &H256) -> Option<BlockNumber>;

    fn block(&self, hash: &H256) -> Option<Block>;

    fn genesis_hash(&self) -> H256;

    fn get_transaction(&self, hash: &H256) -> Option<Transaction>;

    fn contain_transaction(&self, hash: &H256) -> bool;

    fn get_transaction_meta(&self, output_root: &H256, hash: &H256) -> Option<TransactionMeta>;

    fn get_transaction_meta_at(&self, hash: &H256, parent: &H256) -> Option<TransactionMeta>;

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

    fn output_root(&self, hash: &H256) -> Option<H256> {
        self.store.get_output_root(hash)
    }

    fn get_transaction(&self, hash: &H256) -> Option<Transaction> {
        self.store.get_transaction(hash)
    }

    fn contain_transaction(&self, hash: &H256) -> bool {
        self.store.get_transaction_address(hash).is_some()
    }

    fn get_transaction_meta(&self, output_root: &H256, hash: &H256) -> Option<TransactionMeta> {
        self.store.get_transaction_meta(*output_root, *hash)
    }

    fn get_transaction_meta_at(&self, hash: &H256, parent: &H256) -> Option<TransactionMeta> {
        self.output_root(parent)
            .and_then(|root| self.store.get_transaction_meta(root, *hash))
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

                hash = self.block_header(&hash).unwrap().parent_hash();
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
    fn calculate_difficulty(&self, last: &Header) -> Option<U256> {
        let last_hash = last.hash();
        let last_number = last.number();
        let last_difficulty = last.difficulty();

        let interval = self.consensus.difficulty_adjustment_interval();

        if (last_number + 1) % interval != 0 {
            return Some(last_difficulty);
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
            let max_difficulty = last_difficulty * 2;
            if difficulty > max_difficulty {
                return Some(max_difficulty);
            }

            if difficulty < min_difficulty {
                return Some(min_difficulty);
            }
            return Some(difficulty);
        }
        None
    }

    fn consensus(&self) -> &Consensus {
        &self.consensus
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
