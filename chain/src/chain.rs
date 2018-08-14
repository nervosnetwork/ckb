use super::{COLUMNS, COLUMN_BLOCK_HEADER};
use bigint::{H256, U256};
use cachedb::CacheDB;
use ckb_notify::{ForkTxs, Notify, MINER_SUBSCRIBER};
use consensus::Consensus;
use core::block::IndexedBlock;
use core::cell::{CellProvider, CellState};
use core::extras::BlockExt;
use core::header::{BlockNumber, IndexedHeader};
use core::transaction::{Capacity, IndexedTransaction, OutPoint, Transaction};
use core::transaction_meta::TransactionMeta;
use db::batch::Batch;
use db::diskdb::RocksDB;
use db::kvdb::KeyValueDB;
use db::memorydb::MemoryKeyValueDB;
use index::ChainIndex;
use log;
use std::cmp;
use std::path::Path;
use store::ChainKVStore;
use time::now_ms;
use util::RwLock;

#[derive(Debug, PartialEq, Clone, Eq)]
pub enum Error {
    InvalidInput,
    InvalidOutput,
}

#[derive(Default, Debug, PartialEq, Clone, Eq)]
pub struct TipHeader {
    pub header: IndexedHeader,
    pub total_difficulty: U256,
    pub output_root: H256,
}

pub struct Chain<CS> {
    store: CS,
    tip_header: RwLock<TipHeader>,
    consensus: Consensus,
    notify: Notify,
}

#[derive(Debug, Clone)]
pub struct BlockInsertionResult {
    pub fork_txs: ForkTxs,
    pub new_best_block: bool,
}

pub fn exclude_miner_sub(name: &str) -> bool {
    name != MINER_SUBSCRIBER
}

pub trait ChainProvider: Sync + Send + CellProvider {
    fn process_block(&self, b: &IndexedBlock, local: bool) -> Result<(), Error>;

    fn block_header(&self, hash: &H256) -> Option<IndexedHeader>;

    fn block_body(&self, hash: &H256) -> Option<Vec<Transaction>>;

    fn block_hash(&self, number: BlockNumber) -> Option<H256>;

    fn block_ext(&self, hash: &H256) -> Option<BlockExt>;

    fn output_root(&self, hash: &H256) -> Option<H256>;

    fn block_number(&self, hash: &H256) -> Option<BlockNumber>;

    fn block(&self, hash: &H256) -> Option<IndexedBlock>;

    fn genesis_hash(&self) -> H256;

    fn consensus(&self) -> &Consensus;

    fn get_ancestor(&self, base: &H256, number: BlockNumber) -> Option<IndexedHeader>;

    //FIXME: This is bad idea
    fn tip_header(&self) -> &RwLock<TipHeader>;

    fn get_transaction(&self, hash: &H256) -> Option<IndexedTransaction>;

    fn contain_transaction(&self, hash: &H256) -> bool;

    fn get_transaction_meta(&self, hash: &H256) -> Option<TransactionMeta>;

    fn get_transaction_meta_at(&self, hash: &H256, parent: &H256) -> Option<TransactionMeta>;

    fn block_reward(&self, block_number: BlockNumber) -> Capacity;

    // Loops through all inputs and outputs of given transaction to calculate
    // fee that miner can obtain. Could result in error state when input
    // transaction is missing.
    fn calculate_transaction_fee(&self, transaction: &Transaction) -> Result<Capacity, Error>;

    fn calculate_difficulty(&self, last: &IndexedHeader) -> Option<U256>;
}

impl<'a, CS: ChainIndex> CellProvider for Chain<CS> {
    fn cell(&self, out_point: &OutPoint) -> CellState {
        let index = out_point.index as usize;
        if let Some(meta) = self.get_transaction_meta(&out_point.hash) {
            if index < meta.len() {
                if !meta.is_spent(index) {
                    let mut transaction = self
                        .store
                        .get_transaction(&out_point.hash)
                        .expect("transaction must exist");
                    return CellState::Head(transaction.outputs.swap_remove(index));
                } else {
                    return CellState::Tail;
                }
            }
        }
        CellState::Unknown
    }

    fn cell_at(&self, out_point: &OutPoint, parent: &H256) -> CellState {
        let index = out_point.index as usize;
        if let Some(meta) = self.get_transaction_meta_at(&out_point.hash, parent) {
            if index < meta.len() {
                if !meta.is_spent(index) {
                    let mut transaction = self
                        .store
                        .get_transaction(&out_point.hash)
                        .expect("transaction must exist");
                    return CellState::Head(transaction.outputs.swap_remove(index));
                } else {
                    return CellState::Tail;
                }
            }
        }
        CellState::Unknown
    }
}

impl<CS: ChainIndex> Chain<CS> {
    pub fn init(store: CS, consensus: Consensus, notify: Notify) -> Result<Chain<CS>, Error> {
        // check head in store or save the genesis block as head
        let header = {
            let genesis = consensus.genesis_block();
            match store.get_tip_header() {
                Some(h) => h,
                None => {
                    store.init(&genesis);
                    genesis.header.clone()
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

        let tip_header = TipHeader {
            header,
            total_difficulty,
            output_root,
        };

        Ok(Chain {
            store,
            consensus,
            tip_header: RwLock::new(tip_header),
            notify,
        })
    }

    fn check_transactions(&self, b: &IndexedBlock) -> Result<H256, Error> {
        let mut cells = Vec::new();

        for tx in &b.transactions {
            let ins = if tx.is_cellbase() {
                Vec::new()
            } else {
                tx.input_pts()
            };
            let outs = tx.output_pts();

            cells.push((ins, outs));
        }

        let root = self
            .output_root(&b.header.parent_hash)
            .ok_or(Error::InvalidOutput)?;

        self.store
            .update_transaction_meta(root, cells)
            .ok_or(Error::InvalidOutput)
    }

    fn insert_block(&self, b: &IndexedBlock, root: H256) -> BlockInsertionResult {
        let mut new_best_block = false;
        let mut old_cumulative_txs = Vec::new();
        let mut new_cumulative_txs = Vec::new();
        self.store.save_with_batch(|batch| {
            let parent_ext = self
                .store
                .get_block_ext(&b.header.parent_hash)
                .expect("parent already store");
            let cannon_total_difficulty = parent_ext.total_difficulty + b.header.difficulty;

            let ext = BlockExt {
                received_at: now_ms(),
                total_difficulty: cannon_total_difficulty,
                total_uncles_count: parent_ext.total_uncles_count + b.uncles().len() as u64,
            };

            self.store.insert_block(batch, b);
            self.store.insert_output_root(batch, b.hash(), root);
            self.store.insert_block_ext(batch, &b.hash(), &ext);

            {
                debug!(target: "chain", "acquire lock");
                let mut tip_header = self.tip_header.write();
                let current_total_difficulty = tip_header.total_difficulty;
                debug!(
                    "difficulty diff = {}; current = {}, cannon = {}",
                    cannon_total_difficulty.low_u64() as i64
                        - current_total_difficulty.low_u64() as i64,
                    current_total_difficulty,
                    cannon_total_difficulty,
                );

                if cannon_total_difficulty > current_total_difficulty
                    || (current_total_difficulty == cannon_total_difficulty
                        && b.hash() < tip_header.header.hash())
                {
                    debug!(target: "chain", "new best block found: {} => {}", b.header().number, b.hash());
                    new_best_block = true;
                    let new_tip_header = TipHeader {
                        header: b.header.clone(),
                        total_difficulty: cannon_total_difficulty,
                        output_root: root,
                    };
                    *tip_header = new_tip_header;
                    self.update_index(batch, b, &mut old_cumulative_txs, &mut new_cumulative_txs);
                    self.store.insert_tip_header(batch, &b.header);
                }
                debug!(target: "chain", "lock release");
            }
        });

        BlockInsertionResult {
            new_best_block,
            fork_txs: ForkTxs(old_cumulative_txs, new_cumulative_txs),
        }
    }

    pub fn notify_insert_result(
        &self,
        b: &IndexedBlock,
        result: BlockInsertionResult,
        local: bool,
    ) {
        let BlockInsertionResult {
            new_best_block,
            fork_txs,
        } = result;
        if !fork_txs.old_txs().is_empty() || !fork_txs.new_txs().is_empty() {
            self.notify
                .notify_switch_fork::<fn(&str) -> bool>(fork_txs, None);
        }

        let filter = if local { Some(exclude_miner_sub) } else { None };

        if new_best_block {
            self.notify.notify_new_tip(b, filter);
            if log_enabled!(target: "chain", log::Level::Debug) {
                self.print_chain(10);
            }
        } else {
            self.notify.notify_side_chain_block(b, filter);
        }
    }

    // we found new best_block total_difficulty > old_chain.total_difficulty
    pub fn update_index(
        &self,
        batch: &mut Batch,
        block: &IndexedBlock,
        old_cumulative_txs: &mut Vec<Transaction>,
        new_cumulative_txs: &mut Vec<Transaction>,
    ) {
        let mut new_block: Option<IndexedBlock> = None;
        loop {
            new_block = {
                let new_block_ref = new_block.as_ref().unwrap_or(block);
                let new_hash = new_block_ref.hash();
                let height = new_block_ref.header().number;

                if let Some(old_hash) = self.block_hash(height) {
                    if new_hash == old_hash {
                        break;
                    }
                    let old_txs = self.block_body(&old_hash).unwrap();
                    self.store.delete_block_hash(batch, height);
                    self.store.delete_block_number(batch, &old_hash);
                    self.store.delete_transaction_address(batch, &old_txs);
                    old_cumulative_txs.extend(old_txs.into_iter().rev());
                }

                self.store.insert_block_hash(batch, height, &new_hash);
                self.store.insert_block_number(batch, &new_hash, height);
                self.store.insert_transaction_address(
                    batch,
                    &new_hash,
                    &new_block_ref.transactions,
                );
                // Current block body not insert into store yet.
                if new_block.is_some() {
                    let new_txs = self.block_body(&new_hash).unwrap();
                    new_cumulative_txs.extend(new_txs.into_iter().rev());
                }

                // NOTE: Block number should be checked, so loop will finally stop.
                //         1. block.number > 0
                //         2. block.number = block.parent.number + 1
                let block = self.block(&new_block_ref.header().parent_hash).unwrap();
                Some(block)
            };
        }
    }

    fn print_chain(&self, len: u64) {
        debug!(target: "chain", "Chain {{");

        let tip = { self.tip_header().read().header.number };
        let bottom = tip - cmp::min(tip, len);

        for number in (bottom..tip + 1).rev() {
            let hash = self
                .block_hash(number)
                .expect(format!("invaild block number({}), tip={}", number, tip).as_str());
            debug!(target: "chain", "   {} => {}", number, hash);
        }

        debug!(target: "chain", "}}");

        // TODO: remove me when block explorer is available
        debug!(target: "chain", "Tx in Head Block {{");
        for transaction in self
            .block_hash(tip)
            .and_then(|hash| self.store.get_block_body(&hash))
            .expect("invalid block number")
        {
            debug!(target: "chain", "   {} => {:?}", transaction.hash(), transaction);
        }
        debug!(target: "chain", "}}");

        debug!(target: "chain", "Uncle block {{");
        for (index, uncle) in self
            .block_hash(tip)
            .and_then(|hash| self.store.get_block_uncles(&hash))
            .expect("invalid block number")
            .iter()
            .enumerate()
        {
            debug!(target: "chain", "   {} => {:#?}", index, uncle);
        }
        debug!(target: "chain", "}}");
    }
}

impl<CS: ChainIndex> ChainProvider for Chain<CS> {
    fn process_block(&self, b: &IndexedBlock, local: bool) -> Result<(), Error> {
        debug!(target: "chain", "begin processing block: {}", b.hash());

        let root = self.check_transactions(b)?;
        let insert_result = self.insert_block(b, root);
        self.notify_insert_result(b, insert_result, local);
        debug!(target: "chain", "finish processing block");
        Ok(())
    }

    fn block(&self, hash: &H256) -> Option<IndexedBlock> {
        self.store.get_block(hash)
    }

    fn block_body(&self, hash: &H256) -> Option<Vec<Transaction>> {
        self.store.get_block_body(hash)
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
        self.consensus.genesis_block().hash()
    }

    fn consensus(&self) -> &Consensus {
        &self.consensus
    }

    fn get_ancestor(&self, base: &H256, number: BlockNumber) -> Option<IndexedHeader> {
        {
            let tip = self.tip_header().read();
            if let Some(n_number) = self.block_number(base) {
                if number > n_number {
                    return None;
                } else if number == n_number {
                    return Some(tip.header.clone());
                } else {
                    return self
                        .block_hash(number)
                        .and_then(|hash| self.block_header(&hash));
                }
            }
        }
        if let Some(header) = self.block_header(base) {
            let mut n_number = header.number;
            let mut index_walk = header;
            if number > n_number {
                return None;
            }

            while n_number > number {
                if let Some(header) = self.block_header(&index_walk.parent_hash) {
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

    fn block_header(&self, hash: &H256) -> Option<IndexedHeader> {
        let tip_header = self.tip_header.read();
        if &tip_header.header.hash() == hash {
            Some(tip_header.header.clone())
        } else {
            self.store.get_header(hash)
        }
    }

    fn tip_header(&self) -> &RwLock<TipHeader> {
        &self.tip_header
    }

    fn output_root(&self, hash: &H256) -> Option<H256> {
        self.store.get_output_root(hash)
    }

    fn get_transaction(&self, hash: &H256) -> Option<IndexedTransaction> {
        self.store.get_transaction(hash)
    }

    fn contain_transaction(&self, hash: &H256) -> bool {
        self.store.get_transaction_address(hash).is_some()
    }

    fn get_transaction_meta(&self, hash: &H256) -> Option<TransactionMeta> {
        self.store
            .get_transaction_meta(self.tip_header.read().output_root, *hash)
    }

    fn get_transaction_meta_at(&self, hash: &H256, parent: &H256) -> Option<TransactionMeta> {
        self.output_root(parent)
            .and_then(|root| self.store.get_transaction_meta(root, *hash))
    }

    fn block_reward(&self, _block_number: BlockNumber) -> Capacity {
        // TODO: block reward calculation algorithm
        self.consensus.initial_block_reward()
    }

    // TODO: find a way to write test for this once we can build a mock on
    // ChainIndex
    fn calculate_transaction_fee(&self, transaction: &Transaction) -> Result<Capacity, Error> {
        let mut fee = 0;
        for input in &transaction.inputs {
            let previous_output = &input.previous_output;
            match self.get_transaction(&previous_output.hash) {
                Some(previous_transaction) => {
                    let index = previous_output.index as usize;
                    if index < previous_transaction.outputs.len() {
                        fee += previous_transaction.outputs[index].capacity;
                    } else {
                        return Err(Error::InvalidInput);
                    }
                }
                None => return Err(Error::InvalidInput),
            }
        }
        let spent_capacity: Capacity = transaction
            .outputs
            .iter()
            .map(|output| output.capacity)
            .sum();
        if spent_capacity > fee {
            return Err(Error::InvalidOutput);
        }
        fee -= spent_capacity;
        Ok(fee)
    }

    // T_interval = L / C_m
    // HR_m = HR_last/ (1 + o)
    // Diff= HR_m * T_interval / H = Diff_last * o_last / o
    fn calculate_difficulty(&self, last: &IndexedHeader) -> Option<U256> {
        let interval = self.consensus().difficulty_adjustment_interval();

        if (last.number + 1) % interval != 0 {
            return Some(last.difficulty);
        }

        let start = last.number.saturating_sub(interval);
        if let Some(start_header) = self.get_ancestor(&last.hash(), start) {
            let start_total_uncles_count = self
                .block_ext(&start_header.hash())
                .expect("block_ext exist")
                .total_uncles_count;

            let last_total_uncles_count = self
                .block_ext(&last.hash())
                .expect("block_ext exist")
                .total_uncles_count;

            let difficulty = last.difficulty
                * U256::from(last_total_uncles_count - start_total_uncles_count)
                * U256::from((1.0 / self.consensus().orphan_rate_target()) as u64)
                / U256::from(interval);

            let min_difficulty = self.consensus().min_difficulty();
            let max_difficulty = last.difficulty * 2;
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
}

pub struct ChainBuilder<CS> {
    store: CS,
    consensus: Consensus,
    notify: Option<Notify>,
}

impl<CS: ChainIndex> ChainBuilder<CS> {
    pub fn new_memory() -> ChainBuilder<ChainKVStore<MemoryKeyValueDB>> {
        let db = MemoryKeyValueDB::open(COLUMNS as usize);
        ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_simple(db)
    }

    pub fn new_rocks<P: AsRef<Path>>(path: P) -> ChainBuilder<ChainKVStore<CacheDB<RocksDB>>> {
        let db = CacheDB::new(
            RocksDB::open(path, COLUMNS),
            &[(COLUMN_BLOCK_HEADER.unwrap(), 4096)],
        );
        ChainBuilder::<ChainKVStore<CacheDB<RocksDB>>>::new_simple(db)
    }

    pub fn new_simple<T: KeyValueDB>(db: T) -> ChainBuilder<ChainKVStore<T>> {
        let mut consensus = Consensus::default();
        consensus.initial_block_reward = 50;
        ChainBuilder {
            store: ChainKVStore { db },
            consensus,
            notify: None,
        }
    }

    // pub fn config(mut self, value: Config) -> Self {
    //     self.config = value;
    //     self
    // }

    // pub fn get_config(&self) -> &Config {
    //     &self.config
    // }

    pub fn consensus(mut self, value: Consensus) -> Self {
        self.consensus = value;
        self
    }

    pub fn get_consensus(&self) -> &Consensus {
        &self.consensus
    }

    pub fn notify(mut self, value: Notify) -> Self {
        self.notify = Some(value);
        self
    }

    pub fn build(self) -> Result<Chain<CS>, Error> {
        let notify = self.notify.unwrap_or_else(Notify::new);
        Chain::init(self.store, self.consensus, notify)
    }
}

#[cfg(test)]
pub mod test {
    use super::*;

    use consensus::GenesisBuilder;
    use core::header::{Header, RawHeader, Seal};
    use core::transaction::{CellInput, CellOutput, Transaction, VERSION};
    use core::uncle::UncleBlock;
    use db::memorydb::MemoryKeyValueDB;
    use store::ChainKVStore;

    fn create_cellbase(number: BlockNumber) -> Transaction {
        let inputs = vec![CellInput::new_cellbase_input(number)];
        let outputs = vec![CellOutput::new(0, vec![], H256::from(0))];
        Transaction::new(VERSION, Vec::new(), inputs, outputs)
    }

    fn gen_block(parent_header: IndexedHeader, nonce: u64, difficulty: U256) -> IndexedBlock {
        let time = now_ms();
        let number = parent_header.number + 1;
        let cellbase = create_cellbase(number);
        let header = Header {
            raw: RawHeader {
                number,
                version: 0,
                parent_hash: parent_header.hash(),
                timestamp: time,
                txs_commit: H256::zero(),
                difficulty: difficulty,
                cellbase_id: H256::zero(),
                uncles_hash: H256::zero(),
            },
            seal: Seal {
                nonce,
                mix_hash: H256::from(nonce),
            },
        };

        IndexedBlock {
            header: header.into(),
            transactions: vec![cellbase],
            uncles: vec![],
        }
    }

    #[test]
    fn test_chain_fork_by_total_difficulty() {
        let chain = ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .build()
            .unwrap();
        let final_number = 20;

        let mut chain1: Vec<IndexedBlock> = Vec::new();
        let mut chain2: Vec<IndexedBlock> = Vec::new();

        let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty;
            let new_block = gen_block(parent, i, difficulty + U256::from(100));
            chain1.push(new_block.clone());
            parent = new_block.header;
        }

        parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty;
            let j = if i > 10 { 110 } else { 99 };
            let new_block = gen_block(parent, i + 1000, difficulty + U256::from(j));
            chain2.push(new_block.clone());
            parent = new_block.header;
        }

        for block in &chain1 {
            chain
                .process_block(&block, false)
                .expect("process block ok");
        }

        for block in &chain2 {
            chain
                .process_block(&block, false)
                .expect("process block ok");
        }
        assert_eq!(chain.block_hash(8), chain2.get(7).map(|b| b.hash()));
    }

    #[test]
    fn test_chain_fork_by_hash() {
        let chain = ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .build()
            .unwrap();
        let final_number = 20;

        let mut chain1: Vec<IndexedBlock> = Vec::new();
        let mut chain2: Vec<IndexedBlock> = Vec::new();

        let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty;
            let new_block = gen_block(parent, i, difficulty + U256::from(100));
            chain1.push(new_block.clone());
            parent = new_block.header;
        }

        parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty;
            let new_block = gen_block(parent, i + 1000, difficulty + U256::from(100));
            chain2.push(new_block.clone());
            parent = new_block.header;
        }

        for block in &chain1 {
            chain
                .process_block(&block, false)
                .expect("process block ok");
        }

        for block in &chain2 {
            chain
                .process_block(&block, false)
                .expect("process block ok");
        }

        //if total_difficulty equal, we chose block which have smaller hash as best
        assert!(
            chain1
                .iter()
                .zip(chain2.iter())
                .all(|(a, b)| a.header.difficulty == b.header.difficulty)
        );

        let best = if chain1[(final_number - 2) as usize].hash()
            < chain2[(final_number - 2) as usize].hash()
        {
            chain1
        } else {
            chain2
        };
        assert_eq!(chain.block_hash(8), best.get(7).map(|b| b.hash()));
        assert_eq!(chain.block_hash(19), best.get(18).map(|b| b.hash()));
    }

    #[test]
    fn test_chain_get_ancestor() {
        let chain = ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .build()
            .unwrap();
        let final_number = 20;

        let mut chain1: Vec<IndexedBlock> = Vec::new();
        let mut chain2: Vec<IndexedBlock> = Vec::new();

        let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty;
            let new_block = gen_block(parent, i, difficulty + U256::from(100));
            chain1.push(new_block.clone());
            parent = new_block.header;
        }

        parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty;
            let new_block = gen_block(parent, i + 1000, difficulty + U256::from(100));
            chain2.push(new_block.clone());
            parent = new_block.header;
        }

        for block in &chain1 {
            chain
                .process_block(&block, false)
                .expect("process block ok");
        }

        for block in &chain2 {
            chain
                .process_block(&block, false)
                .expect("process block ok");
        }

        assert_eq!(
            chain1[9].header,
            chain
                .get_ancestor(&chain1.last().unwrap().hash(), 10)
                .unwrap()
        );

        assert_eq!(
            chain2[9].header,
            chain
                .get_ancestor(&chain2.last().unwrap().hash(), 10)
                .unwrap()
        );
    }

    fn push_uncle(block: &mut IndexedBlock, uncle: &IndexedBlock) {
        let uncle = UncleBlock {
            header: uncle.header.header.clone(),
            cellbase: uncle.transactions.first().cloned().unwrap(),
        };

        block.uncles.push(uncle);
        block.header.uncles_hash = block.cal_uncles_hash();
        block.finalize_dirty();
    }

    #[test]
    fn test_calculate_difficulty() {
        let genesis_builder = GenesisBuilder::default();
        let genesis_block = genesis_builder.difficulty(U256::from(1000)).build();
        let mut consensus = Consensus::default().set_genesis_block(genesis_block);
        consensus.pow_time_span = 200;
        consensus.pow_spacing = 1;

        let chain = ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .consensus(consensus.clone())
            .build()
            .unwrap();
        let final_number = chain.consensus().difficulty_adjustment_interval();

        let mut chain1: Vec<IndexedBlock> = Vec::new();
        let mut chain2: Vec<IndexedBlock> = Vec::new();

        let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number - 1 {
            let difficulty = chain.calculate_difficulty(&parent).unwrap();
            let new_block = gen_block(parent, i, difficulty);
            chain
                .process_block(&new_block, false)
                .expect("process block ok");
            chain1.push(new_block.clone());
            parent = new_block.header;
        }

        parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = chain.calculate_difficulty(&parent).unwrap();
            let mut new_block = gen_block(parent, i + 100, difficulty);
            if i < 26 {
                push_uncle(&mut new_block, &chain1[i as usize]);
            }
            chain
                .process_block(&new_block, false)
                .expect("process block ok");
            chain2.push(new_block.clone());
            parent = new_block.header;
        }
        let tip = { chain.tip_header().read().header.clone() };
        let total_uncles_count = chain.block_ext(&tip.hash()).unwrap().total_uncles_count;
        assert_eq!(total_uncles_count, 25);
        let difficulty = chain.calculate_difficulty(&tip).unwrap();

        // 25 * 10 * 1000 / 200
        assert_eq!(difficulty, U256::from(1250));

        let chain = ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .consensus(consensus.clone())
            .build()
            .unwrap();
        let mut chain2: Vec<IndexedBlock> = Vec::new();
        for i in 1..final_number - 1 {
            chain
                .process_block(&chain1[(i - 1) as usize], false)
                .expect("process block ok");
        }

        parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = chain.calculate_difficulty(&parent).unwrap();
            let mut new_block = gen_block(parent, i + 100, difficulty);
            if i < 11 {
                push_uncle(&mut new_block, &chain1[i as usize]);
            }
            chain
                .process_block(&new_block, false)
                .expect("process block ok");
            chain2.push(new_block.clone());
            parent = new_block.header;
        }
        let tip = { chain.tip_header().read().header.clone() };
        let total_uncles_count = chain.block_ext(&tip.hash()).unwrap().total_uncles_count;
        assert_eq!(total_uncles_count, 10);
        let difficulty = chain.calculate_difficulty(&tip).unwrap();

        // min[10 * 10 * 1000 / 200, 1000]
        assert_eq!(difficulty, U256::from(1000));

        let chain = ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .consensus(consensus.clone())
            .build()
            .unwrap();
        let mut chain2: Vec<IndexedBlock> = Vec::new();
        for i in 1..final_number - 1 {
            chain
                .process_block(&chain1[(i - 1) as usize], false)
                .expect("process block ok");
        }

        parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = chain.calculate_difficulty(&parent).unwrap();
            let mut new_block = gen_block(parent, i + 100, difficulty);
            if i < 151 {
                push_uncle(&mut new_block, &chain1[i as usize]);
            }
            chain
                .process_block(&new_block, false)
                .expect("process block ok");
            chain2.push(new_block.clone());
            parent = new_block.header;
        }
        let tip = { chain.tip_header().read().header.clone() };
        let total_uncles_count = chain.block_ext(&tip.hash()).unwrap().total_uncles_count;
        assert_eq!(total_uncles_count, 150);
        let difficulty = chain.calculate_difficulty(&tip).unwrap();

        // max[150 * 10 * 1000 / 200, 2 * 1000]
        assert_eq!(difficulty, U256::from(2000));
    }
}
