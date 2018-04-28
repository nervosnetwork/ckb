extern crate bigint;
extern crate bincode;
extern crate nervos_core as core;
extern crate nervos_time as time;
extern crate nervos_util as util;
extern crate rand;
extern crate serde;
#[macro_use]
extern crate serde_derive;

mod tests;
pub mod txs_pool;

use bigint::H256;
use core::block::Block;
use std::collections::{HashMap, HashSet};
use util::RwLock;

pub use txs_pool::*;

#[derive(Default)]
pub struct OrphanBlockPool {
    pool: RwLock<HashMap<H256, Vec<Block>>>,
    hashes: RwLock<HashMap<H256, H256>>,
}

impl OrphanBlockPool {
    pub fn add_block(&self, b: Block) -> Option<H256> {
        {
            if self.hashes.read().contains_key(&b.hash()) {
                return None;
            }
        }
        let pre_hash = b.header.pre_hash;
        {
            let mut pool = self.pool.write();
            let mut hashes = self.hashes.write();

            hashes.insert(b.hash(), pre_hash);
            let blocks = pool.entry(pre_hash).or_insert_with(Vec::new);
            blocks.push(b);
        }
        Some(self.tail_hash(pre_hash))
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
            for b in &blocks {
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
            .iter()
            .filter(|b| b.header.timestamp <= t)
            .cloned()
            .collect();
        let lt: Vec<Block> = self.pool
            .read()
            .iter()
            .filter(|b| b.header.timestamp > t)
            .cloned()
            .collect();
        *self.pool.write() = lt;

        let mut hashes = self.hashes.write();
        for b in &bt {
            hashes.remove(&b.hash());
        }
        bt
    }
}
