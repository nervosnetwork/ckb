use bigint::{H256, U256};
use config::Config;
use core::block::Block;
use core::cell::{CellProvider, CellState};
use core::extras::BlockExt;
use core::header::Header;
use core::transaction::{OutPoint, Transaction};
use core::transaction_meta::TransactionMeta;
use db::batch::Batch;
use ethash::Ethash;
use index::ChainIndex;
use std::sync::Arc;
use time::now_ms;
use util::{Mutex, RwLock, RwLockReadGuard};

#[derive(Debug, PartialEq, Clone, Eq)]
pub enum Error {
    InvalidInput,
    InvalidOutput,
}

pub enum SealerType {
    Normal,
    Noop,
}

pub struct Chain<CS> {
    store: CS,
    config: Config,
    tip_header: RwLock<Header>,
    total_difficulty: RwLock<U256>,
    output_root: RwLock<H256>,
    ethash: Option<Arc<Ethash>>,
    lock: Mutex<()>,
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

    fn get_transaction_meta(&self, hash: &H256) -> Option<TransactionMeta>;

    // NOTE: reward and fee are returned now as u32 since capacity is also
    // u32 in protocol, we might want to revisit this later
    fn block_reward(&self, block_number: u64) -> u32;

    // Loops through all inputs and outputs of given transaction to calculate
    // fee that miner can obtain. Could result in error state when input
    // transaction is missing.
    fn calculate_transaction_fee(&self, transaction: &Transaction) -> Result<u32, Error>;

    fn sealer_type(&self) -> SealerType;

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
            let _guard = self.lock.lock();

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
                cannon_total_difficulty > current_total_difficulty
            };

            if best_block {
                info!(target: "chain", "new best block found: {}", b.hash());
                *self.total_difficulty.write() = cannon_total_difficulty;
                self.update_index(batch, &b);
                *self.tip_header.write() = b.header.clone();
                self.store.insert_tip_header(batch, &b.header);
                *self.output_root.write() = root;
            }
        });
        self.print_chain(b.header.number, 10);
    }

    // we found new best_block total_difficulty > old_chain.total_difficulty
    pub fn update_index(&self, batch: &mut Batch, b: &Block) {
        let old_height = { self.tip_header.read().number };
        let mut height = b.header.number - 1;

        if height < old_height {
            for h in height..old_height + 1 {
                let hash = self.block_hash(h).unwrap();
                let txs = self.block_body(&hash).unwrap();
                self.store.delete_block_hash(batch, h);
                self.store.delete_block_height(batch, &hash);
                self.store.delete_transaction_address(batch, &txs);
            }
        }

        self.store
            .insert_block_hash(batch, b.header.number, &b.hash());
        self.store
            .insert_block_height(batch, &b.hash(), b.header.number);
        self.store
            .insert_transaction_address(batch, &b.hash(), &b.transactions);

        let mut hash = b.header.parent_hash;

        loop {
            if let Some(old_hash) = self.block_hash(height) {
                if old_hash == hash {
                    break;
                }
                let txs = self.block_body(&old_hash).unwrap();
                self.store.delete_transaction_address(batch, &txs);
            }

            let txs = self.block_body(&hash).unwrap();
            self.store.insert_block_hash(batch, height, &hash);
            self.store.insert_block_height(batch, &hash, height);
            self.store.insert_transaction_address(batch, &hash, &txs);

            hash = self.block_header(&hash).unwrap().parent_hash;
            height -= 1;
        }
    }

    fn print_chain(&self, tip: u64, len: u64) {
        info!(target: "chain", "Chain {{");

        let limit = if tip > len { len } else { tip } + 1;

        for i in 0..limit {
            let hash = self.block_hash(tip - i).expect("invaild block number");
            info!(target: "chain", "   {} => {}", tip - i, hash);
        }

        info!(target: "chain", "}}");

        // TODO: remove me when block explorer is available
        info!(target: "chain", "Tx in Head Block {{");
        for transaction in self
            .block_hash(tip)
            .and_then(|hash| self.store.get_block_body(&hash))
            .expect("invalid block number")
        {
            info!(target: "chain", "   {} => {:?}", transaction.hash(), transaction);
        }
        info!(target: "chain", "}}");
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
        info!(target: "chain", "begin processing block: {}", b.hash());
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

    fn sealer_type(&self) -> SealerType {
        if self.config.sealer_type == "Normal" {
            SealerType::Normal
        } else {
            SealerType::Noop
        }
    }

    fn ethash(&self) -> Option<Arc<Ethash>> {
        self.ethash.clone()
    }
}
