use bigint::H256;
use core::block::Block;
use core::cell::{CellProvider, CellState};
use core::header::Header;
use core::transaction::{OutPoint, Transaction};
use nervos_chain::chain::{ChainClient, Error, TransactionMeta};
use std::collections::HashMap;
use util::RwLock;
use util::RwLockReadGuard;

/// Dummy adapter used as a placeholder for real implementations
// TODO: do we need this dummy, if it's never used?
// pub struct NoopAdapter {}
// impl PoolAdapter for NoopAdapter {
//     fn tx_accepted(&self, _: &Transaction) {}
// }

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
    head: RwLock<Header>,
}

impl DummyChainImpl {
    pub fn new() -> DummyChainImpl {
        DummyChainImpl {
            output: RwLock::new(DummyOutputSet::new()),
            headers: RwLock::new(Vec::new()),
            head: RwLock::new(Header::default()),
        }
    }
}

impl CellProvider for DummyChainImpl {
    fn cell(&self, o: &OutPoint) -> CellState {
        self.output
            .read()
            .get_output(o)
            .map(|x| match x {
                &Some(_) => CellState::Tail,
                &None => CellState::Head(Default::default()),
            })
            .unwrap_or(CellState::Unknown)
    }
}

impl ChainClient for DummyChainImpl {
    fn process_block(&self, _b: &Block) -> Result<(), Error> {
        Ok(())
    }

    fn get_locator(&self) -> Vec<H256> {
        vec![]
    }

    fn block_header(&self, _hash: &H256) -> Option<Header> {
        None
    }

    fn block_hash(&self, _height: u64) -> Option<H256> {
        None
    }

    fn block_height(&self, _hash: &H256) -> Option<u64> {
        None
    }

    fn block(&self, _hash: &H256) -> Option<Block> {
        None
    }

    //FIXME: This is bad idea
    fn head_header(&self) -> RwLockReadGuard<Header> {
        self.head.read()
    }

    fn get_transaction(&self, _hash: &H256) -> Option<Transaction> {
        None
    }

    fn get_transaction_meta(&self, _hash: &H256) -> Option<TransactionMeta> {
        None
    }

    fn block_body(&self, _hash: &H256) -> Option<Vec<Transaction>> {
        None
    }

    fn output_root(&self, _hash: &H256) -> Option<H256> {
        None
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

pub trait DummyChain: ChainClient {
    fn update_output_set(&mut self, new_output: DummyOutputSet);
    fn apply_block(&self, b: &Block);
    fn store_head_header(&self, header: &Header);
}
