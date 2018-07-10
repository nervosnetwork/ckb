extern crate bigint;
extern crate crossbeam_channel;
extern crate nervos_chain;
extern crate nervos_core as core;
extern crate nervos_notify;
extern crate nervos_time as time;
extern crate nervos_util as util;
extern crate nervos_verification;
#[macro_use]
extern crate serde_derive;
#[cfg(test)]
extern crate ethash;

mod tests;
pub mod txs_pool;

use bigint::H256;
use core::block::Block;
// use core::cell::{CellProvider, CellState};
// use core::transaction::{OutPoint, Transaction};
use std::collections::hash_map::Entry;
use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet, VecDeque};
use util::RwLock;

pub use txs_pool::*;

#[derive(Default)]
pub struct OrphanBlockPool {
    blocks: RwLock<HashMap<H256, HashMap<H256, Block>>>,
}

impl OrphanBlockPool {
    pub fn with_capacity(capacity: usize) -> Self {
        OrphanBlockPool {
            blocks: RwLock::new(HashMap::with_capacity(capacity)),
        }
    }

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
    pool: RwLock<BTreeMap<u64, Block>>,
    hashes: RwLock<HashSet<H256>>,
}

impl PendingBlockPool {
    pub fn with_capacity(capacity: usize) -> Self {
        PendingBlockPool {
            pool: RwLock::new(BTreeMap::new()),
            hashes: RwLock::new(HashSet::with_capacity(capacity)),
        }
    }

    pub fn add_block(&self, b: Block) -> bool {
        let hashes = self.hashes.upgradable_read();
        let exists = !hashes.contains(&b.hash());
        if exists {
            let mut write_hashes = hashes.upgrade();
            write_hashes.insert(b.hash());
            self.pool.write().insert(b.header.timestamp, b);
        }
        exists
    }

    pub fn get_block(&self, t: u64) -> Vec<Block> {
        use std::mem::swap;
        let mut lt = self.pool.write();
        let mut hashes = self.hashes.write();
        let mut bt = lt.split_off(&t);
        swap(&mut bt, &mut lt);

        let bt: Vec<_> = bt.into_iter().map(|(_k, v)| v).collect();

        for b in &bt {
            hashes.remove(&b.hash());
        }
        bt
    }

    pub fn len(&self) -> usize {
        self.pool.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
