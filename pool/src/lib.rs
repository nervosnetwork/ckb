extern crate bigint;
extern crate bincode;
extern crate nervos_chain;
extern crate nervos_core as core;
extern crate nervos_notify;
extern crate nervos_time as time;
extern crate nervos_util as util;
extern crate nervos_verification;
extern crate rand;
extern crate serde;
#[macro_use]
extern crate serde_derive;

mod tests;
pub mod txs_pool;

use bigint::H256;
use core::block::Block;
// use core::cell::{CellProvider, CellState};
// use core::transaction::{OutPoint, Transaction};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet, VecDeque};
use util::RwLock;

pub use txs_pool::*;

#[derive(Default)]
pub struct OrphanBlockPool {
    blocks: RwLock<HashMap<H256, HashMap<H256, Block>>>,
}

impl OrphanBlockPool {
    /// Insert orphaned block, for which we have already requested its parent block
    pub fn insert(&self, block: Block) {
        self.blocks
            .write()
            .entry(block.header.parent_hash)
            .or_insert_with(HashMap::new)
            .insert(block.hash(), block);
    }

    pub fn remove_blocks_by_parent(&self, hash: &H256) -> VecDeque<Block> {
        let mut queue: VecDeque<H256> = VecDeque::new();
        queue.push_back(*hash);

        let mut removed: VecDeque<Block> = VecDeque::new();
        while let Some(parent_hash) = queue.pop_front() {
            if let Entry::Occupied(entry) = self.blocks.write().entry(parent_hash) {
                let (_, orphaned) = entry.remove_entry();
                queue.extend(orphaned.keys().cloned());
                removed.extend(orphaned.into_iter().map(|(_, b)| b));
            }
        }
        removed
    }

    pub fn len(&self) -> usize {
        self.blocks.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
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
        let bt: Vec<Block> = self
            .pool
            .read()
            .iter()
            .filter(|b| b.header.timestamp <= t)
            .cloned()
            .collect();
        let lt: Vec<Block> = self
            .pool
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
