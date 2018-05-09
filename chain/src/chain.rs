use super::genesis::genesis_hash;
use bigint::H256;
use core::block::Block;
use core::cell::{CellProvider, CellState};
use core::difficulty::cal_difficulty;
use core::header::Header;
use core::transaction::{OutPoint, Transaction};
pub use core::transaction_meta::TransactionMeta;
use rand::{thread_rng, Rng};
use store::ChainStore;
use util::{Mutex, RwLock, RwLockReadGuard};

#[derive(Debug)]
pub enum Error {
    Duplicate,
    InvalidBlockTime,
    InvalidBlockHeight,
    InvalidChallenge,
    InvalidDifficulty,
    InvalidTotalDifficulty,
    InvalidBlockHash,
    InvalidOutput,
    NotFound,
}

#[derive(Debug)]
pub struct Chain<CS> {
    store: CS,
    head_header: RwLock<Header>,
    output_root: RwLock<H256>,
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
    fn head_header(&self) -> RwLockReadGuard<Header>;

    fn get_transaction(&self, hash: &H256) -> Option<Transaction>;

    fn get_transaction_meta(&self, hash: &H256) -> Option<TransactionMeta>;
}

impl<CS: ChainStore> CellProvider for Chain<CS> {
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

impl<CS: ChainStore> Chain<CS> {
    pub fn init(store: CS, genesis: &Block) -> Result<Chain<CS>, Error> {
        // check head in store or save the genesis block as head
        let head_header = match store.head_header() {
            Some(h) => h,
            None => {
                store.init(genesis);
                genesis.header.clone()
            }
        };

        let r = match store.get_output_root(head_header.hash()) {
            Some(h) => h,
            None => H256::zero(),
        };

        Ok(Chain {
            store,
            head_header: RwLock::new(head_header),
            output_root: RwLock::new(r),
            lock: Mutex::new(()),
        })
    }

    // TODO: validate transactions in block
    fn check_header(&self, h: &Header) -> Result<(), Error> {
        if self.block_header(&h.hash()).is_some() {
            return Err(Error::Duplicate);
        }

        let pre_header = self.block_header(&h.parent_hash).unwrap();

        if pre_header.height + 1 != h.height {
            return Err(Error::InvalidBlockHeight);
        }

        // if pre_header.timestamp / TIME_STEP >= h.timestamp / TIME_STEP {
        //     return Err(Error::InvalidBlockTime);
        // }

        // if h.total_difficulty != pre_header.total_difficulty + h.difficulty {
        //     return Err(Error::InvalidTotalDifficulty);
        // }

        if cal_difficulty(&pre_header, h.timestamp) != h.difficulty {
            return Err(Error::InvalidDifficulty);
        }

        // if self.challenge(&pre_header) != Some(h.challenge) {
        //     return Err(Error::InvalidChallenge);
        // }

        Ok(())
    }

    fn check_transactions(&self, b: &Block) -> Result<H256, Error> {
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();

        for tx in &b.transactions {
            let mut ins = tx.input_pts();
            let mut outs = tx.output_pts();

            inputs.append(&mut ins);
            outputs.append(&mut outs);
        }

        let root = self.output_root(&b.header.parent_hash).unwrap();

        self.store
            .update_transaction_meta(root, inputs, outputs)
            .ok_or(Error::InvalidOutput)
    }

    //TODO: best block
    fn insert_block(&self, b: &Block, root: H256) {
        self.store.save_block(b);
        self.store.save_output_root(b.hash(), root);

        // let best_block = {
        //     let head_header = self.head_header.read();
        //     // if b.header.height == head_header.height {
        //     //     let mut rng = thread_rng();
        //     //     b.header.difficulty > head_header.difficulty
        //     //         || (b.header.difficulty == head_header.difficulty && rng.gen_range(0, 2) == 0)
        //     // } else

        //     // b.header.difficulty > head_header.difficulty
        //     //     || (b.header.difficulty == head_header.difficulty && rng.gen_range(0, 2) == 0)
        // };
        let best_block = {
            let head_header = self.head_header.read();
            if b.header.height == head_header.height {
                let mut rng = thread_rng();
                b.header.difficulty > head_header.difficulty
                    || (b.header.difficulty == head_header.difficulty && rng.gen_range(0, 2) == 0)
            } else {
                b.header.height == head_header.height + 1
            }
        };

        if best_block {
            info!(target: "chain", "new best block found: {}", b.hash());
            let _guard = self.lock.lock();
            self.update_main_chain(&b);
            self.save_head_header(&b.header);
            *self.output_root.write() = root;
        }
    }

    fn save_head_header(&self, h: &Header) {
        let mut head_header = self.head_header.write();
        *head_header = h.clone();
        self.store.save_head_header(h);
    }

    // fn ancestor_hash(&self, height: u64, header: &Header) -> Option<H256> {
    //     if header.height < height {
    //         return None;
    //     }

    //     if header.height == height {
    //         return Some(header.hash());
    //     }

    //     let mut current_hash = header.parent_hash;
    //     let mut current_height = header.height - 1;

    //     while current_height > height {
    //         let hash = self.block_hash(current_height).unwrap();
    //         if hash == current_hash {
    //             return self.block_hash(height);
    //         }
    //         current_hash = self.block_header(&current_hash).unwrap().parent_hash;
    //         current_height -= 1;
    //     }

    //     Some(current_hash)
    // }

    // fn ancestor_header(&self, height: u64, header: &Header) -> Option<Header> {
    //     self.ancestor_hash(height, header)
    //         .and_then(|v| self.block_header(&v))
    // }

    pub fn update_main_chain(&self, b: &Block) {
        let old_height = { self.head_header.read().height };
        let mut height = b.header.height - 1;

        if height < old_height {
            for h in height..old_height + 1 {
                let hash = self.block_hash(h).unwrap();
                let txs = self.block_body(&hash).unwrap();
                self.store.delete_block_hash(h);
                self.store.delete_transaction_address(&txs);
            }
        }

        self.store.save_block_hash(b.header.height, &b.hash());
        self.store
            .save_transaction_address(&b.hash(), &b.transactions);

        let mut hash = b.header.parent_hash;

        loop {
            if let Some(old_hash) = self.block_hash(height) {
                if old_hash == hash {
                    break;
                }
                let txs = self.block_body(&old_hash).unwrap();
                self.store.delete_transaction_address(&txs);
            }

            let txs = self.block_body(&hash).unwrap();
            self.store.save_block_hash(height, &hash);
            self.store.save_transaction_address(&hash, &txs);

            hash = self.block_header(&hash).unwrap().parent_hash;
            height -= 1;
        }

        self.print_chain(b.header.height, 10);
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

impl<CS: ChainStore> ChainClient for Chain<CS> {
    fn get_locator(&self) -> Vec<H256> {
        let mut step = 1;
        let mut locator = Vec::with_capacity(32);
        let header = self.head_header.read();
        let mut index = header.height;
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
                    locator.push(genesis_hash())
                }
                break;
            }
            index -= step;
        }
        locator
    }

    fn process_block(&self, b: &Block) -> Result<(), Error> {
        info!(target: "chain", "begin processing block: {}", b.hash());
        self.check_header(&b.header)?;
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
        self.block_header(hash).map(|v| v.height)
    }

    fn block_header(&self, hash: &H256) -> Option<Header> {
        let head_header = self.head_header.read();
        if &head_header.hash() == hash {
            Some(head_header.clone())
        } else {
            self.store.get_header(hash)
        }
    }

    fn head_header(&self) -> RwLockReadGuard<Header> {
        self.head_header.read()
    }

    fn output_root(&self, hash: &H256) -> Option<H256> {
        self.store.get_output_root(*hash)
    }

    fn get_transaction(&self, hash: &H256) -> Option<Transaction> {
        self.store.get_transaction(hash)
    }

    fn get_transaction_meta(&self, hash: &H256) -> Option<TransactionMeta> {
        self.store
            .get_transaction_meta(*self.output_root.read(), *hash)
    }
}
