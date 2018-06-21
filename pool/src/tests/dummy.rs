use bigint::H256;
use core::block::IndexedBlock;
use core::cell::{CellProvider, CellState};
use core::extras::BlockExt;
use core::header::IndexedHeader;
use core::transaction::{IndexedTransaction, OutPoint, Transaction};
use core::transaction_meta::TransactionMeta;
use nervos_chain::chain::TipHeader;
use nervos_chain::chain::{ChainProvider, Error};
use std::collections::HashMap;
use util::RwLock;

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

    pub fn with_block(&mut self, b: &IndexedBlock) {
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
    headers: RwLock<Vec<IndexedHeader>>,
    tip: RwLock<TipHeader>,
}

impl DummyChainImpl {
    pub fn new() -> DummyChainImpl {
        DummyChainImpl {
            output: RwLock::new(DummyOutputSet::new()),
            headers: RwLock::new(Vec::new()),
            tip: RwLock::new(TipHeader::default()),
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

impl ChainProvider for DummyChainImpl {
    fn process_block(&self, _b: &IndexedBlock) -> Result<(), Error> {
        Ok(())
    }

    fn contain_transaction(&self, _h: &H256) -> bool {
        false
    }

    fn block_header(&self, _hash: &H256) -> Option<IndexedHeader> {
        None
    }

    fn block_hash(&self, _number: u64) -> Option<H256> {
        None
    }

    fn block_number(&self, _hash: &H256) -> Option<u64> {
        None
    }

    fn block(&self, _hash: &H256) -> Option<IndexedBlock> {
        None
    }

    fn tip_header(&self) -> &RwLock<TipHeader> {
        &self.tip
    }

    fn genesis_hash(&self) -> H256 {
        H256::zero()
    }

    fn get_transaction(&self, _hash: &H256) -> Option<IndexedTransaction> {
        None
    }

    fn block_ext(&self, _hash: &H256) -> Option<BlockExt> {
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

    fn block_reward(&self, _block_number: u64) -> u32 {
        0
    }

    fn calculate_transaction_fee(&self, _transaction: &Transaction) -> Result<u32, Error> {
        Ok(0)
    }
}

impl DummyChain for DummyChainImpl {
    fn update_output_set(&mut self, new_output: DummyOutputSet) {
        self.output = RwLock::new(new_output);
    }

    fn apply_block(&self, b: &IndexedBlock) {
        self.output.write().with_block(b);
        self.store_tip_header(&b.header)
    }

    fn store_tip_header(&self, header: &IndexedHeader) {
        let mut headers = self.headers.write();
        headers.insert(0, header.clone());
    }
}

pub trait DummyChain: ChainProvider {
    fn update_output_set(&mut self, new_output: DummyOutputSet);
    fn apply_block(&self, b: &IndexedBlock);
    fn store_tip_header(&self, header: &IndexedHeader);
}
