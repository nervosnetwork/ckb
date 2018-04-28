use std::collections::HashMap;
use util::RwLock;

use core::block::{Block, Header};
use core::transaction::{OutPoint, Transaction};

use bigint::H256;

use txs_pool::types::{BlockChain, Parent, PoolAdapter};

/// Dummy adapter used as a placeholder for real implementations
// TODO: do we need this dummy, if it's never used?
pub struct NoopAdapter {}
impl PoolAdapter for NoopAdapter {
    fn tx_accepted(&self, _: &Transaction) {}
}

/// A DummyOutputSet for mocking up the chain
#[derive(Debug, PartialEq, Clone)]
pub struct DummyOutputSet {
    outputs: HashMap<OutPoint, Option<H256>>,
}

impl DummyOutputSet {
    pub fn new() -> DummyOutputSet {
        DummyOutputSet {
            outputs: HashMap::new(),
        }
    }

    pub fn with_block(&mut self, b: &Block) {
        let txs = &b.transactions;

        for tx in txs {
            let h = tx.hash();
            let inputs = tx.input_pts();
            let outputs = tx.output_pts();

            for i in inputs {
                self.outputs.insert(i, Some(h));
            }

            for o in outputs {
                self.outputs.insert(o, None);
            }
        }
    }

    pub fn get_output(&self, o: &OutPoint) -> Option<&Option<H256>> {
        self.outputs.get(o)
    }

    // only for testing: add an output to the map
    pub fn with_output(&self, o: OutPoint) -> DummyOutputSet {
        let mut new_outputs = self.outputs.clone();
        new_outputs.insert(o, None);
        DummyOutputSet {
            outputs: new_outputs,
        }
    }
}

/// A DummyChain is the mocked chain for playing with what methods we would
/// need
pub struct DummyChainImpl {
    output: RwLock<DummyOutputSet>,
    headers: RwLock<Vec<Header>>,
}

impl DummyChainImpl {
    pub fn new() -> DummyChainImpl {
        DummyChainImpl {
            output: RwLock::new(DummyOutputSet::new()),
            headers: RwLock::new(Vec::new()),
        }
    }
}

impl BlockChain for DummyChainImpl {
    fn is_spent(&self, o: &OutPoint) -> Option<Parent> {
        self.output.read().get_output(o).map(|x| match x {
            &Some(_) => Parent::AlreadySpent,
            &None => Parent::BlockTransaction,
        })
    }

    fn head_header(&self) -> Option<Header> {
        let headers = self.headers.read();
        if headers.len() > 0 {
            Some(headers[0].clone())
        } else {
            None
        }
    }
}

impl DummyChain for DummyChainImpl {
    fn update_output_set(&mut self, new_output: DummyOutputSet) {
        self.output = RwLock::new(new_output);
    }

    fn apply_block(&self, b: &Block) {
        self.output.write().with_block(b);
        self.store_head_header(&b.header)
    }

    fn store_head_header(&self, header: &Header) {
        let mut headers = self.headers.write();
        headers.insert(0, header.clone());
    }
}

pub trait DummyChain: BlockChain {
    fn update_output_set(&mut self, new_output: DummyOutputSet);
    fn apply_block(&self, b: &Block);
    fn store_head_header(&self, header: &Header);
}
