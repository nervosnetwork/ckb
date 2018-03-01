extern crate bigint;
extern crate nervos_core as core;

use bigint::H256;
use core::block::Block;
use core::transaction::Transaction;
use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Default)]
pub struct TransactionPool {
    pool: RwLock<HashMap<H256, Transaction>>,
}

impl TransactionPool {
    pub fn add_transaction(&self, tx: Transaction) {
        let mut pool = self.pool.write().unwrap();
        pool.insert(tx.hash(), tx);
        ()
    }
    pub fn get_transactions(&self, limit: usize) -> Vec<Transaction> {
        let pool = self.pool.read().unwrap();
        pool.iter().take(limit).map(|(_, tx)| tx).cloned().collect()
    }
}

pub struct OrphanBlockPool {}

impl OrphanBlockPool {
    pub fn add_block(&self, _b: &Block) {}
}
