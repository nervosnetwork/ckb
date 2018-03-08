extern crate bigint;
extern crate nervos_core as core;
extern crate nervos_util as util;

use bigint::H256;
use core::block::Block;
use core::transaction::Transaction;
use std::collections::{HashMap, HashSet};
use util::RwLock;

#[derive(Default)]
pub struct TransactionPool {
    pool: RwLock<HashMap<H256, Transaction>>,
}

impl TransactionPool {
    pub fn add_transaction(&self, tx: Transaction) {
        let mut pool = self.pool.write();
        pool.insert(tx.hash(), tx);
        ()
    }
    pub fn get_transactions(&self, limit: usize) -> Vec<Transaction> {
        let pool = self.pool.read();
        pool.iter().take(limit).map(|(_, tx)| tx).cloned().collect()
    }
    /// Updates the pool with the details of a new block.
    pub fn accommodate(&self, _block: &Block) {
        // TODO implement this
    }
}

#[derive(Default)]
pub struct OrphanBlockPool {
    pool: RwLock<HashMap<H256, Vec<Block>>>,
    hashes: RwLock<HashMap<H256, H256>>,
}

impl OrphanBlockPool {
    pub fn add_block(&self, b: Block) -> Option<H256> {
        if self.hashes.read().contains_key(&b.hash()) {
            None
        } else {
            let mut pool = self.pool.write();
            let mut hashes = self.hashes.write();
            let pre_hash = b.header.pre_hash;

            hashes.insert(b.hash(), pre_hash);
            let blocks = pool.entry(pre_hash).or_insert_with(Vec::new);
            blocks.push(b);

            Some(self.tail_hash(pre_hash))
        }
    }

    pub fn tail_hash(&self, mut hash: H256) -> H256 {
        let hashes = self.hashes.read();

        while let Some(h) = hashes.get(&hash) {
            hash = *h;
        }

        hash
    }

    pub fn remove_block(&self, h: &H256) -> Vec<Block> {
        if let Some(blocks) = self.pool.write().remove(h) {
            let mut hashes = self.hashes.write();
            for b in blocks.clone() {
                hashes.remove(&b.hash());
            }

            blocks
        } else {
            Vec::new()
        }
    }

    pub fn contains(&self, h: &H256) -> bool {
        self.hashes.read().contains_key(h)
    }
}

#[derive(Default)]
pub struct PendingBlockPool {
    pool: RwLock<Vec<Block>>,
    hashes: RwLock<HashSet<H256>>,
}

impl PendingBlockPool {
    pub fn add_block(&self, b: Block) -> bool {
        let v = { !self.hashes.read().contains(&b.hash()) };
        if v {
            self.hashes.write().insert(b.hash());
            self.pool.write().push(b);
        }
        v
    }

    pub fn get_block(&self, t: u64) -> Vec<Block> {
        let bt: Vec<Block> = self.pool
            .read()
            .clone()
            .into_iter()
            .filter(|b| b.header.timestamp <= t)
            .collect();
        let lt: Vec<Block> = self.pool
            .read()
            .clone()
            .into_iter()
            .filter(|b| b.header.timestamp > t)
            .collect();
        *self.pool.write() = lt;

        let mut hashes = self.hashes.write();
        for b in bt.clone() {
            hashes.remove(&b.hash());
        }
        bt
    }
}
