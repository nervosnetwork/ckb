extern crate bigint;
extern crate crossbeam_channel;
extern crate nervos_chain;
extern crate nervos_core as core;
#[cfg(test)]
extern crate nervos_db;
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
use core::block::IndexedBlock;
// use core::cell::{CellProvider, CellState};
// use core::transaction::{OutPoint, Transaction};
use std::collections::BTreeMap;
use std::collections::HashSet;
use util::{RwLock, RwLockUpgradableReadGuard};

pub use txs_pool::*;

#[derive(Default)]
pub struct PendingBlockPool {
    pool: RwLock<BTreeMap<u64, IndexedBlock>>,
    hashes: RwLock<HashSet<H256>>,
}

impl PendingBlockPool {
    pub fn with_capacity(capacity: usize) -> Self {
        PendingBlockPool {
            pool: RwLock::new(BTreeMap::new()),
            hashes: RwLock::new(HashSet::with_capacity(capacity)),
        }
    }

    pub fn add_block(&self, b: IndexedBlock) -> bool {
        let hashes = self.hashes.upgradable_read();
        let exists = !hashes.contains(&b.hash());
        if exists {
            let mut write_hashes = RwLockUpgradableReadGuard::upgrade(hashes);
            write_hashes.insert(b.hash());
            self.pool.write().insert(b.header.timestamp, b);
        }
        exists
    }

    pub fn get_block(&self, t: u64) -> Vec<IndexedBlock> {
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
