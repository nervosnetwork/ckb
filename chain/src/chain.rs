use super::genesis::genesis_hash;
use bigint::H256;
use core::block::{Block, Header};
use core::cell::{CellProvider, CellState};
use core::difficulty::cal_difficulty;
use core::global::{EPOCH_LEN, HEIGHT_SHIFT, TIME_STEP};
use core::transaction::OutPoint;
use db::store::ChainStore;
use rand::{thread_rng, Rng};
use util::{RwLock, RwLockReadGuard};

#[derive(Debug)]
pub enum Error {
    Duplicate,
    InvalidBlockTime,
    InvalidBlockHeight,
    InvalidChallenge,
    InvalidDifficulty,
    InvalidTotalDifficulty,
    InvalidBlockHash,
    NotFound,
}

#[derive(Debug)]
pub struct Chain<CS> {
    store: CS,
    head_header: RwLock<Header>,
}

pub trait ChainClient: Sync + Send {
    fn process_block(&self, b: &Block) -> Result<(), Error>;

    fn get_locator(&self) -> Vec<H256>;

    fn block_header(&self, hash: &H256) -> Option<Header>;

    fn block_hash(&self, height: u64) -> Option<H256>;

    fn block_height(&self, hash: &H256) -> Option<u64>;

    fn block(&self, hash: &H256) -> Option<Block>;

    fn head_header(&self) -> RwLockReadGuard<Header>;

    fn challenge(&self, pre_header: &Header) -> Option<H256>;
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
        Ok(Chain {
            store,
            head_header: RwLock::new(head_header),
        })
    }

    // TODO: validate transactions in block
    fn check_block(&self, h: &Header) -> Result<(), Error> {
        if self.block_header(&h.hash()).is_some() {
            return Err(Error::Duplicate);
        }

        let pre_header = self.block_header(&h.pre_hash).unwrap();

        if pre_header.height + 1 != h.height {
            return Err(Error::InvalidBlockHeight);
        }

        if pre_header.timestamp / TIME_STEP >= h.timestamp / TIME_STEP {
            return Err(Error::InvalidBlockTime);
        }

        if h.total_difficulty != pre_header.total_difficulty + h.difficulty {
            return Err(Error::InvalidTotalDifficulty);
        }

        if cal_difficulty(&pre_header, h.timestamp) != h.difficulty {
            return Err(Error::InvalidDifficulty);
        }

        if self.challenge(&pre_header) != Some(h.challenge) {
            return Err(Error::InvalidChallenge);
        }

        Ok(())
    }

    fn insert_block(&self, b: &Block) {
        self.store.save_block(b);

        let best_block = {
            let head_header = self.head_header.read();
            let mut rng = thread_rng();
            b.header.total_difficulty > head_header.total_difficulty
                || (b.header.total_difficulty == head_header.total_difficulty
                    && rng.gen_range(0, 2) == 0)
        };

        if best_block {
            info!(target: "chain", "new best block found: {}", b.hash());
            self.save_head_header(&b.header);
        }
    }

    fn save_head_header(&self, h: &Header) {
        let mut head_header = self.head_header.write();
        *head_header = h.clone();
        self.store.save_head_header(h);
        self.print_chain(h.height, 10);
    }

    fn ancestor_hash(&self, height: u64, header: &Header) -> Option<H256> {
        if header.height < height {
            return None;
        }

        if header.height == height {
            return Some(header.hash());
        }

        let mut current_hash = header.pre_hash;
        let mut current_height = header.height - 1;

        while current_height > height {
            let hash = self.block_hash(current_height).unwrap();
            if hash == current_hash {
                return self.block_hash(height);
            }
            current_hash = self.block_header(&current_hash).unwrap().pre_hash;
            current_height -= 1;
        }

        Some(current_hash)
    }

    fn ancestor_header(&self, height: u64, header: &Header) -> Option<Header> {
        self.ancestor_hash(height, header)
            .and_then(|v| self.block_header(&v))
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
        for transaction in self.block_hash(tip)
            .and_then(|hash| self.store.get_block_transactions(&hash))
            .expect("invalid block number")
        {
            info!(target: "chain", "   {} => {:?}", transaction.hash(), transaction);
        }
        info!(target: "chain", "}}");
    }
}

impl<CS: ChainStore> CellProvider for Chain<CS> {
    fn cell(&self, out_point: &OutPoint) -> CellState {
        let index = out_point.index as usize;
        if let Some(meta) = self.store.get_transaction_meta(&out_point.hash) {
            if index < meta.spent_at.len() {
                if !meta.is_spent(index) {
                    let mut transaction = self.store
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

impl<CS: ChainStore> ChainClient for Chain<CS> {
    fn get_locator(&self) -> Vec<H256> {
        let mut step = 1;
        let mut locator = Vec::with_capacity(32);
        let header = self.head_header.read();
        let mut index = header.height;
        loop {
            let block_hash = self.block_hash(index)
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
        self.check_block(&b.header)?;
        self.insert_block(b);
        info!(target: "chain", "finish processing block");
        Ok(())
    }

    fn block(&self, hash: &H256) -> Option<Block> {
        self.store.get_block(hash)
    }

    fn block_hash(&self, height: u64) -> Option<H256> {
        self.store.get_block_hash(height)
    }

    fn block_height(&self, hash: &H256) -> Option<u64> {
        self.store.get_block_height(hash)
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

    fn challenge(&self, pre_header: &Header) -> Option<H256> {
        let height = pre_header.height + 1;

        if height % EPOCH_LEN != 0 {
            return Some(pre_header.challenge);
        }

        let pick_height = if height < HEIGHT_SHIFT {
            0
        } else {
            height - HEIGHT_SHIFT
        };

        self.ancestor_header(pick_height, pre_header)
            .map(|v| v.proof.hash())
    }
}
