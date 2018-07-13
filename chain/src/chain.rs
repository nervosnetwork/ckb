use super::{COLUMNS, COLUMN_BLOCK_HEADER};
use bigint::{H256, U256};
use cachedb::CacheDB;
use config::Config;
use core::block::Block;
use core::cell::{CellProvider, CellState};
use core::extras::BlockExt;
use core::header::Header;
use core::transaction::{OutPoint, Transaction};
use core::transaction_meta::TransactionMeta;
use db::batch::Batch;
use db::diskdb::RocksDB;
use db::kvdb::KeyValueDB;
use db::memorydb::MemoryKeyValueDB;
use ethash::Ethash;
use index::ChainIndex;
use nervos_notify::Notify;
use std::cmp;
use std::path::Path;
use std::sync::Arc;
use store::ChainKVStore;
use time::now_ms;
use util::{Mutex, RwLock, RwLockReadGuard};

#[derive(Debug, PartialEq, Clone, Eq)]
pub enum Error {
    InvalidInput,
    InvalidOutput,
}

pub enum VerificationLevel {
    /// Full verification.
    Full,
    /// Transaction scripts are not checked.
    Header,
    /// No verification at all.
    NoVerification,
}

pub struct Chain<CS> {
    store: CS,
    config: Config,
    tip_header: RwLock<Header>,
    total_difficulty: RwLock<U256>,
    output_root: RwLock<H256>,
    ethash: Option<Arc<Ethash>>,
    lock: Mutex<()>,
    notify: Notify,
}

pub trait ChainClient: Sync + Send + CellProvider {
    fn process_block(&self, b: &Block) -> Result<(), Error>;

    fn get_locator(&self) -> Vec<H256>;

    fn block_header(&self, hash: &H256) -> Option<Header>;

    fn block_body(&self, hash: &H256) -> Option<Vec<Transaction>>;

    fn block_hash(&self, height: u64) -> Option<H256>;

    fn output_root(&self, hash: &H256) -> Option<H256>;

    fn block_height(&self, hash: &H256) -> Option<u64>;

    fn block(&self, hash: &H256) -> Option<Block>;

    //FIXME: This is bad idea
    fn tip_header(&self) -> RwLockReadGuard<Header>;

    fn get_transaction(&self, hash: &H256) -> Option<Transaction>;

    fn contain_transaction(&self, hash: &H256) -> bool;

    fn get_transaction_meta(&self, hash: &H256) -> Option<TransactionMeta>;

    // NOTE: reward and fee are returned now as u32 since capacity is also
    // u32 in protocol, we might want to revisit this later
    fn block_reward(&self, block_number: u64) -> u32;

    // Loops through all inputs and outputs of given transaction to calculate
    // fee that miner can obtain. Could result in error state when input
    // transaction is missing.
    fn calculate_transaction_fee(&self, transaction: &Transaction) -> Result<u32, Error>;

    fn verification_level(&self) -> VerificationLevel;

    fn ethash(&self) -> Option<Arc<Ethash>>;
}

impl<CS: ChainIndex> CellProvider for Chain<CS> {
    fn cell(&self, out_point: &OutPoint) -> CellState {
        let index = out_point.index as usize;
        if let Some(meta) = self.get_transaction_meta(&out_point.hash) {
            if meta.is_fully_spent() {
                return CellState::Unknown;
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
    pub fn init(
        store: CS,
        config: Config,
        ethash: Option<Arc<Ethash>>,
        notify: Notify,
    ) -> Result<Chain<CS>, Error> {
        // check head in store or save the genesis block as head
        let genesis = config.genesis_block();
        let tip_header = match store.get_tip_header() {
            Some(h) => h,
            None => {
                store.init(&genesis);
                genesis.header.clone()
            }
        };

        let r = match store.get_output_root(&tip_header.hash()) {
            Some(h) => h,
            None => H256::zero(),
        };

        let td = store
            .get_block_ext(&tip_header.hash())
            .expect("block_ext stored")
            .total_difficulty;

        Ok(Chain {
            store,
            config,
            ethash,
            tip_header: RwLock::new(tip_header),
            output_root: RwLock::new(r),
            total_difficulty: RwLock::new(td),
            lock: Mutex::new(()),
            notify,
        })
    }

    fn check_transactions(&self, b: &Block) -> Result<H256, Error> {
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();

        for tx in &b.transactions {
            let mut ins = tx.input_pts();
            let outs = tx.output_pts();

            // Cellbase transaction only has one null input
            if !tx.is_cellbase() {
                inputs.append(&mut ins);
            }
            outputs.push(outs);
        }

        let root = self.output_root(&b.header.parent_hash).unwrap();

        self.store
            .update_transaction_meta(root, inputs, outputs)
            .ok_or(Error::InvalidOutput)
    }

    //TODO: best block
    fn insert_block(&self, b: &Block, root: H256) {
        self.store.save_with_batch(|batch| {
            let block_hash = b.hash();
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
            self.store.insert_output_root(batch, block_hash, root);
            self.store.insert_block_ext(batch, &b.hash(), &ext);

            let best_block = {
                let current_total_difficulty = *self.total_difficulty.read();
                info!(
                    "difficulty diff = {}; current = {}, cannon = {}",
                    cannon_total_difficulty.low_u64() as i64
                        - current_total_difficulty.low_u64() as i64,
                    current_total_difficulty,
                    cannon_total_difficulty,
                );
                cannon_total_difficulty > current_total_difficulty
            };

            if best_block {
                let _guard = self.lock.lock();
                info!(target: "chain", "new best block found: {} => {}", b.header().number, b.hash());
                self.notify.notify_canon_block(b.clone());
                *self.total_difficulty.write() = cannon_total_difficulty;
                self.update_index(batch, &b);
                *self.tip_header.write() = b.header.clone();
                self.store.insert_tip_header(batch, &b.header);
                *self.output_root.write() = root;
            }
        });
        self.print_chain(10);
    }

    // we found new best_block total_difficulty > old_chain.total_difficulty
    pub fn update_index(&self, batch: &mut Batch, block: &Block) {
        let mut new_block: Option<Block> = None;
        let mut old_cumulative_txs = Vec::new();
        let mut new_cumulative_txs = Vec::new();
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
                    self.store.delete_block_height(batch, &old_hash);
                    self.store.delete_transaction_address(batch, &old_txs);
                    old_cumulative_txs.extend(old_txs.into_iter().rev());
                }

                self.store.insert_block_hash(batch, height, &new_hash);
                self.store.insert_block_height(batch, &new_hash, height);
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

        if !old_cumulative_txs.is_empty() || !new_cumulative_txs.is_empty() {
            self.notify
                .notify_switch_fork((old_cumulative_txs, new_cumulative_txs));
        }
    }

    fn print_chain(&self, len: u64) {
        debug!(target: "chain", "Chain {{");

        let tip = self.tip_header().number;
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

impl<CS: ChainIndex> ChainClient for Chain<CS> {
    fn get_locator(&self) -> Vec<H256> {
        let mut step = 1;
        let mut locator = Vec::with_capacity(32);
        let header = self.tip_header.read();
        let mut index = header.number;
        loop {
            let block_hash = self
                .block_hash(index)
                .expect("index calculated in get_locator");
            locator.push(block_hash);

            if locator.len() >= 10 {
                step <<= 1;
            }

            if index < step {
                // always include genesis hash
                if index != 0 {
                    locator.push(self.config.hash)
                }
                break;
            }
            index -= step;
        }
        locator
    }

    fn process_block(&self, b: &Block) -> Result<(), Error> {
        info!(target: "chain", "begin processing block: {} => {}", b.header().number, b.hash());
        // TODO move avl check to verifier??
        let root = self.check_transactions(b)?;
        self.insert_block(b, root);
        info!(target: "chain", "finish processing block");
        Ok(())
    }

    fn block(&self, hash: &H256) -> Option<Block> {
        self.store.get_block(hash)
    }

    fn block_body(&self, hash: &H256) -> Option<Vec<Transaction>> {
        self.store.get_block_body(hash)
    }

    fn block_hash(&self, height: u64) -> Option<H256> {
        self.store.get_block_hash(height)
    }

    fn block_height(&self, hash: &H256) -> Option<u64> {
        self.store.get_block_height(hash)
    }

    fn block_header(&self, hash: &H256) -> Option<Header> {
        let tip_header = self.tip_header.read();
        if &tip_header.hash() == hash {
            Some(tip_header.clone())
        } else {
            self.store.get_header(hash)
        }
    }

    fn tip_header(&self) -> RwLockReadGuard<Header> {
        self.tip_header.read()
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

    fn get_transaction_meta(&self, hash: &H256) -> Option<TransactionMeta> {
        self.store
            .get_transaction_meta(*self.output_root.read(), *hash)
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

    fn verification_level(&self) -> VerificationLevel {
        if self.config.verification_level == "Full" {
            VerificationLevel::Full
        } else if self.config.verification_level == "Header" {
            VerificationLevel::Header
        } else {
            VerificationLevel::NoVerification
        }
    }

    fn ethash(&self) -> Option<Arc<Ethash>> {
        self.ethash.clone()
    }
}

pub struct ChainBuilder<'a, CS> {
    store: CS,
    config: Config,
    ethash: Option<&'a Arc<Ethash>>,
    notify: Option<Notify>,
}

impl<'a, CS: ChainIndex> ChainBuilder<'a, CS> {
    pub fn new_memory() -> ChainBuilder<'a, ChainKVStore<MemoryKeyValueDB>> {
        let db = MemoryKeyValueDB::open(COLUMNS as usize);
        ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_simple(db)
    }

    pub fn new_rocks<P: AsRef<Path>>(path: P) -> ChainBuilder<'a, ChainKVStore<CacheDB<RocksDB>>> {
        let db = CacheDB::new(
            RocksDB::open(path, COLUMNS),
            &[(COLUMN_BLOCK_HEADER.unwrap(), 4096)],
        );
        ChainBuilder::<ChainKVStore<CacheDB<RocksDB>>>::new_simple(db)
    }

    pub fn new_simple<T: KeyValueDB>(db: T) -> ChainBuilder<'a, ChainKVStore<T>> {
        let mut config = Config::default();
        config.initial_block_reward = 50;
        ChainBuilder {
            store: ChainKVStore { db },
            config,
            ethash: None,
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

    pub fn verification_level(mut self, value: &str) -> Self {
        self.config.verification_level = value.to_string();
        self
    }

    pub fn ethash(mut self, value: &'a Arc<Ethash>) -> Self {
        self.ethash = Some(value);
        self
    }

    pub fn notify(mut self, value: Notify) -> Self {
        self.notify = Some(value);
        self
    }

    pub fn build(self) -> Result<Chain<CS>, Error> {
        let notify = self.notify.unwrap_or_else(Notify::new);
        let ethash = self.ethash.map(Arc::clone);
        Chain::init(self.store, self.config, ethash, notify)
    }
}

#[cfg(test)]
pub mod test {
    use super::*;

    use core::header::{Header, RawHeader, Seal};
    use db::memorydb::MemoryKeyValueDB;
    use ethash::Ethash;
    use store::ChainKVStore;
    use tempdir::TempDir;

    fn gen_block<CS: ChainIndex>(
        chain: &Chain<CS>,
        parent_header: Option<Header>,
        nonce: u64,
        difficulty: u64,
        number: u64,
    ) -> Block {
        let parent_header = parent_header.unwrap_or_else(|| {
            let parent_hash = chain.block_hash(number - 1).unwrap();
            chain.block_header(&parent_hash).unwrap()
        });
        let time = now_ms();
        let header = Header {
            raw: RawHeader {
                number,
                version: 0,
                parent_hash: parent_header.hash(),
                timestamp: time,
                txs_commit: H256::from(0),
                difficulty: U256::from(100000 + difficulty),
            },
            seal: Seal {
                nonce,
                mix_hash: H256::from(nonce),
            },
            hash: None,
        };

        Block {
            header,
            transactions: vec![],
        }
    }

    #[test]
    fn test_chain_fork() {
        let tmp_dir = TempDir::new("").unwrap();
        let ethash = Arc::new(Ethash::new(tmp_dir.path()));
        let chain = ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .ethash(&ethash)
            .verification_level("NoVerification")
            .build()
            .unwrap();
        let final_number = 10;

        // block mined from local
        for i in 1..final_number {
            println!("insert block number = {}", i);
            let new_block = gen_block(&chain, None, i, i * 100, i);
            chain.process_block(&new_block).expect("process block ok");
            assert!(chain.block_hash(i).is_some());
        }
        // block sync from remote (bigger difficulty)
        for i in 1..final_number {
            println!("insert block number = {}", i);
            let new_block = gen_block(&chain, None, 1000 + i, i * 200, i);
            chain.process_block(&new_block).expect("process block ok");
            assert!(chain.block_hash(i).is_some());
        }
    }
}
