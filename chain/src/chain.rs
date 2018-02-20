use adapter::ChainAdapter;
use core::block::{Block, Header};
use std::sync::Arc;
use store::ChainStore;

#[derive(Debug)]
pub enum Error {
    InvalidPow,
    InvalidBlockTime,
    InvalidBlockHeight,
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
}
