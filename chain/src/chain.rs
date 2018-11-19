use super::{COLUMNS, COLUMN_BLOCK_HEADER};
use bigint::{H256, U256};
use cachedb::CacheDB;
use config::Config;
use core::block::IndexedBlock;
use core::cell::{CellProvider, CellState};
use core::extras::BlockExt;
use core::header::IndexedHeader;
use core::transaction::{IndexedTransaction, OutPoint, Transaction};
use core::transaction_meta::TransactionMeta;
use db::batch::Batch;
use db::diskdb::RocksDB;
use db::kvdb::KeyValueDB;
use db::memorydb::MemoryKeyValueDB;
use index::ChainIndex;
use log;
use nervos_notify::Notify;
use std::cmp;
use std::path::Path;
use store::ChainKVStore;
use time::now_ms;
use util::RwLock;

pub struct GenesisHash(pub H256);

// guarantee inly init once
unsafe impl Sync for GenesisHash {}

#[derive(Debug, PartialEq, Clone, Eq)]
pub enum Error {
    InvalidInput,
    InvalidOutput,
}

#[derive(Debug, PartialEq, Clone, Eq)]
pub enum VerificationLevel {
    /// Full verification.
    Full,
    /// Transaction scripts are not checked.
    Header,
    /// No verification at all.
    NoVerification,
}

#[derive(Default, Debug, PartialEq, Clone, Eq)]
pub struct TipHeader {
    pub header: IndexedHeader,
    pub total_difficulty: U256,
    pub output_root: H256,
}

pub struct Chain<CS> {
    store: CS,
    config: Config,
    tip_header: RwLock<TipHeader>,
    genesis_hash: GenesisHash,
    notify: Notify,
}

pub trait ChainProvider: Sync + Send + CellProvider {
    fn process_block(&self, b: &IndexedBlock) -> Result<(), Error>;

    fn block_header(&self, hash: &H256) -> Option<IndexedHeader>;

    fn block_body(&self, hash: &H256) -> Option<Vec<Transaction>>;

    fn block_hash(&self, number: u64) -> Option<H256>;

    fn block_ext(&self, hash: &H256) -> Option<BlockExt>;

    fn output_root(&self, hash: &H256) -> Option<H256>;

    fn block_number(&self, hash: &H256) -> Option<u64>;

    fn block(&self, hash: &H256) -> Option<IndexedBlock>;

    fn genesis_hash(&self) -> H256;

    //FIXME: This is bad idea
    fn tip_header(&self) -> &RwLock<TipHeader>;

    fn get_transaction(&self, hash: &H256) -> Option<IndexedTransaction>;

    fn contain_transaction(&self, hash: &H256) -> bool;

    fn get_transaction_meta(&self, hash: &H256) -> Option<TransactionMeta>;

    fn get_transaction_meta_at(&self, hash: &H256, parent: &H256) -> Option<TransactionMeta>;

    // NOTE: reward and fee are returned now as u32 since capacity is also
    // u32 in protocol, we might want to revisit this later
    fn block_reward(&self, block_number: u64) -> u32;

    // Loops through all inputs and outputs of given transaction to calculate
    // fee that miner can obtain. Could result in error state when input
    // transaction is missing.
    fn calculate_transaction_fee(&self, transaction: &Transaction) -> Result<u32, Error>;
}

impl<'a, CS: ChainIndex> CellProvider for Chain<CS> {
    fn cell(&self, out_point: &OutPoint) -> CellState {
        let index = out_point.index as usize;
        if let Some(meta) = self.get_transaction_meta(&out_point.hash) {
            if meta.is_fully_spent() {
                return CellState::Tail;
            }

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
            if meta.is_fully_spent() {
                return CellState::Tail;
            }

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
    pub fn init(store: CS, config: Config, notify: Notify) -> Result<Chain<CS>, Error> {
        // check head in store or save the genesis block as head
        let genesis = config.genesis_block();
        let header = match store.get_tip_header() {
            Some(h) => h,
            None => {
                store.init(&genesis);
                genesis.header.clone()
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
            config,
            genesis_hash: GenesisHash(genesis.hash()),
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

    fn insert_block(&self, b: &IndexedBlock, root: H256) {
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
            };

            self.store.insert_block(batch, b);
            self.store.insert_output_root(batch, b.hash(), root);
            self.store.insert_block_ext(batch, &b.hash(), &ext);

            {
                debug!(target: "chain", "acquire lock");
                let mut tip_header = self.tip_header.write();
                let current_total_difficulty = tip_header.total_difficulty;
                info!(
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
                    info!(target: "chain", "new best block found: {} => {}", b.header().number, b.hash());
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
        if !old_cumulative_txs.is_empty() || !new_cumulative_txs.is_empty() {
            self.notify
                .notify_switch_fork((old_cumulative_txs, new_cumulative_txs));
        }
        if new_best_block {
            self.notify.notify_canon_block(b.clone());
            if log_enabled!(target: "chain", log::Level::Debug) {
                self.print_chain(10);
            }
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
    }
}

impl<CS: ChainIndex> ChainProvider for Chain<CS> {
    fn process_block(&self, b: &IndexedBlock) -> Result<(), Error> {
        info!(target: "chain", "begin processing block: {}", b.hash());
        // self.check_header(&b.header)?;
        let root = self.check_transactions(b)?;
        self.insert_block(b, root);
        info!(target: "chain", "finish processing block");
        Ok(())
    }

    fn block(&self, hash: &H256) -> Option<IndexedBlock> {
        self.store.get_block(hash)
    }

    fn block_body(&self, hash: &H256) -> Option<Vec<Transaction>> {
        self.store.get_block_body(hash)
    }

    fn block_hash(&self, number: u64) -> Option<H256> {
        self.store.get_block_hash(number)
    }

    fn block_ext(&self, hash: &H256) -> Option<BlockExt> {
        self.store.get_block_ext(hash)
    }

    fn block_number(&self, hash: &H256) -> Option<u64> {
        self.store.get_block_number(hash)
    }

    fn genesis_hash(&self) -> H256 {
        self.genesis_hash.0
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

    fn block_reward(&self, _block_number: u64) -> u32 {
        // TODO: block reward calculation algorithm
        self.config.initial_block_reward
    }

    // TODO: find a way to write test for this once we can build a mock on
    // ChainIndex
    fn calculate_transaction_fee(&self, transaction: &Transaction) -> Result<u32, Error> {
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
        let spent_capacity: u32 = transaction
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
}

pub struct ChainBuilder<CS> {
    store: CS,
    config: Config,
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
        let mut config = Config::default();
        config.initial_block_reward = 50;
        ChainBuilder {
            store: ChainKVStore { db },
            config,
            notify: None,
        }
    }

    pub fn config(mut self, value: Config) -> Self {
        self.config = value;
        self
    }

    pub fn get_config(&self) -> &Config {
        &self.config
    }

    pub fn notify(mut self, value: Notify) -> Self {
        self.notify = Some(value);
        self
    }

    pub fn build(self) -> Result<Chain<CS>, Error> {
        let notify = self.notify.unwrap_or_else(Notify::new);
        Chain::init(self.store, self.config, notify)
    }
}

#[cfg(test)]
pub mod test {
    use super::*;

    use core::header::{Header, RawHeader, Seal};
    use db::memorydb::MemoryKeyValueDB;
    use store::ChainKVStore;
    use tempdir::TempDir;

    fn gen_block(
        parent_header: IndexedHeader,
        nonce: u64,
        difficulty: U256,
        number: u64,
    ) -> IndexedBlock {
        let time = now_ms();
        let header = Header {
            raw: RawHeader {
                number,
                version: 0,
                parent_hash: parent_header.hash(),
                timestamp: time,
                txs_commit: H256::from(0),
                difficulty: difficulty,
            },
            seal: Seal {
                nonce,
                mix_hash: H256::from(nonce),
            },
        };

        IndexedBlock {
            header: header.into(),
            transactions: vec![],
        }
    }

    #[test]
    fn test_chain_fork_by_total_difficulty() {
        let _tmp_dir = TempDir::new("test_chain_fork_by_total_difficulty").unwrap();
        let chain = ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .build()
            .unwrap();
        let final_number = 20;

        let mut chain1: Vec<IndexedBlock> = Vec::new();
        let mut chain2: Vec<IndexedBlock> = Vec::new();

        let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty;
            let new_block = gen_block(parent, i, difficulty + U256::from(100), i);
            chain1.push(new_block.clone());
            parent = new_block.header;
        }

        parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty;
            let j = if i > 10 { 110 } else { 99 };
            let new_block = gen_block(parent, i + 1000, difficulty + U256::from(j), i);
            chain2.push(new_block.clone());
            parent = new_block.header;
        }

        for block in &chain1 {
            chain.process_block(&block).expect("process block ok");
        }

        for block in &chain2 {
            chain.process_block(&block).expect("process block ok");
        }
        assert_eq!(chain.block_hash(8), chain2.get(7).map(|b| b.hash()));
    }

    #[test]
    fn test_chain_fork_by_hash() {
        let _tmp_dir = TempDir::new("test_chain_fork_by_hash").unwrap();
        let chain = ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .build()
            .unwrap();
        let final_number = 20;

        let mut chain1: Vec<IndexedBlock> = Vec::new();
        let mut chain2: Vec<IndexedBlock> = Vec::new();

        let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty;
            let new_block = gen_block(parent, i, difficulty + U256::from(100), i);
            chain1.push(new_block.clone());
            parent = new_block.header;
        }

        parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty;
            let new_block = gen_block(parent, i + 1000, difficulty + U256::from(100), i);
            chain2.push(new_block.clone());
            parent = new_block.header;
        }

        for block in &chain1 {
            chain.process_block(&block).expect("process block ok");
        }

        for block in &chain2 {
            chain.process_block(&block).expect("process block ok");
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
}
