use bigint::{H256, U256};
use core::adapter::ChainAdapter;
use core::block::{Block, Header};
use core::difficulty::calculate_difficulty;
use core::global::{EPOCH_LEN, HEIGHT_SHIFT};
use std::sync::Arc;
use store::ChainStore;

#[derive(Debug)]
pub enum Error {
    InvalidPow,
    InvalidBlockTime,
    InvalidBlockHeight,
    NotFound,
}

pub struct Chain {
    store: Arc<ChainStore>,
    adapter: Arc<ChainAdapter>,
}

impl Chain {
    pub fn init(
        store: Arc<ChainStore>,
        adapter: Arc<ChainAdapter>,
        genesis: &Block,
    ) -> Result<Chain, Error> {
        // check head in store or save the genesis block as head
        if store.head_header() == None {
            store.init(genesis);
        }
        Ok(Chain {
            store: store,
            adapter: adapter,
        })
    }

    pub fn process_block(&self, b: &Block) {
        self.store.save_block(b);
        self.adapter.block_accepted(b);
    }

    pub fn head_header(&self) -> Header {
        self.store.head_header().unwrap()
    }

    pub fn block_header(&self, hash: &H256) -> Option<Header> {
        self.store.get_header(hash)
    }

    pub fn block_hash(&self, height: u64) -> Option<H256> {
        self.store.get_block_hash(height)
    }

    pub fn ancestor_hash(&self, height: u64, header: &Header) -> Option<H256> {
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

    pub fn ancestor_header(&self, height: u64, header: &Header) -> Option<Header> {
        self.ancestor_hash(height, header)
            .and_then(|v| self.block_header(&v))
    }

    pub fn challenge(&self, pre_header: &Header) -> Option<H256> {
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

    pub fn cal_difficulty(&self, pre_header: &Header) -> U256 {
        let parent = self.block_header(&pre_header.hash()).unwrap();
        calculate_difficulty(pre_header, &parent)
    }
}
