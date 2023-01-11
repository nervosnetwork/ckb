//! FeeEstimator statistics

use crate::helper::PrettyDisplay;
use crate::types;
use ckb_types::packed::Byte32;
use std::{
    collections::{HashMap, VecDeque},
    time::Duration,
};

#[derive(Default)]
struct BlockTable {
    data: HashMap<Byte32, types::Block>,
    ts2hash_index: HashMap<Duration, Byte32>,
}

#[derive(Default)]
struct TransactionTable {
    data: HashMap<Byte32, types::Transaction>,
    ts2hash_index: VecDeque<(Duration, Byte32)>,
}

// TODO Persistence
#[derive(Default)]
pub(crate) struct Statistics {
    current_number: u64,
    blocks: BlockTable,
    txs: TransactionTable,
    lifetime_dur: Duration,
}

impl BlockTable {
    pub(crate) fn insert(&mut self, block: &types::Block) {
        let block_dt = Duration::from_millis(block.timestamp());
        let hash = block.hash();
        ckb_logger::trace!(
            "insert block#{} {:#x} ({}) into statistics",
            block.number(),
            hash,
            block_dt.pretty()
        );
        self.data.insert(hash.clone(), block.clone());
        self.ts2hash_index.insert(block_dt, hash);
    }

    pub(crate) fn expire(
        &mut self,
        current_dt: Duration,
        lifetime_dur: Duration,
    ) -> Vec<(Duration, types::Block)> {
        let expired_dt = current_dt - lifetime_dur;
        let expired_dt_vec = {
            let mut tmp = self
                .ts2hash_index
                .keys()
                .filter(|&ts| *ts < expired_dt)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            tmp.sort();
            tmp
        };
        let expired_dt_count = expired_dt_vec.len();
        let mut blocks = Vec::with_capacity(expired_dt_count);
        for ts in expired_dt_vec {
            if let Some(hash) = self.ts2hash_index.remove(&ts) {
                if let Some(block) = self.data.remove(&hash) {
                    blocks.push((ts, block));
                }
            }
        }
        ckb_logger::trace!(
            "remove {} (found {}) expired blocks (before {}) from statistics",
            blocks.len(),
            expired_dt_count,
            expired_dt.pretty()
        );
        blocks
    }

    pub(crate) fn filter<F, C, T>(&self, func_filter: F, func_convert: C) -> Vec<T>
    where
        F: Fn(Duration) -> bool,
        C: Fn(Duration, &types::Block) -> Option<T>,
    {
        let ts2hash_vec = self
            .ts2hash_index
            .iter()
            .filter(|(ts, _)| func_filter(**ts))
            .map(|(ts, hash)| (*ts, hash.to_owned()))
            .collect::<Vec<_>>();
        let mut ret = Vec::with_capacity(ts2hash_vec.len());
        for (ts, hash) in ts2hash_vec {
            if let Some(tx) = self.data.get(&hash) {
                if let Some(item) = func_convert(ts, tx) {
                    ret.push(item);
                }
            }
        }
        ret
    }
}

impl TransactionTable {
    pub(crate) fn insert(&mut self, tx: &types::Transaction) {
        let hash = tx.hash();
        ckb_logger::trace!(
            "insert transaction {:#x} ({}) into statistics",
            hash,
            tx.seen_dt().pretty()
        );
        self.data.insert(hash.clone(), tx.to_owned());
        self.ts2hash_index.push_front((tx.seen_dt(), hash));
    }

    pub(crate) fn remove(&mut self, hash: &Byte32) -> Option<types::Transaction> {
        let tx_opt = self.data.remove(hash);
        let ts_idx_opt = self
            .ts2hash_index
            .iter()
            .enumerate()
            .filter(|(_, (_, h))| h == hash)
            .map(|(idx, _)| idx)
            .next();
        if let Some(index) = ts_idx_opt {
            self.ts2hash_index.remove(index);
        }
        if let Some(ref tx) = tx_opt {
            ckb_logger::trace!(
                "remove transaction {} ({}) from statistics",
                hash,
                tx.seen_dt().pretty()
            );
        }
        tx_opt
    }

    pub(crate) fn expire(
        &mut self,
        current_dt: Duration,
        lifetime_dur: Duration,
    ) -> Vec<types::Transaction> {
        let expired_dt = current_dt - lifetime_dur;
        let expired_count = self
            .ts2hash_index
            .iter()
            .rev()
            .skip_while(|(ts, _)| *ts > expired_dt)
            .count();
        let expired_index = self.ts2hash_index.len() - expired_count;
        let expired_hashes = self.ts2hash_index.drain(expired_index..);
        let mut txs = Vec::with_capacity(expired_hashes.len());
        for (_, hash) in expired_hashes {
            if let Some(tx) = self.data.remove(&hash) {
                txs.push(tx);
            }
        }
        ckb_logger::trace!(
            "remove {} (found {}) expired transactions (before {}) from statistics",
            txs.len(),
            expired_count,
            expired_dt.pretty(),
        );
        txs
    }

    pub(crate) fn filter<F, C, T>(&self, func_filter: F, func_convert: C) -> Vec<T>
    where
        F: Fn(Duration) -> bool,
        C: Fn(Duration, &types::Transaction) -> Option<T>,
    {
        let ts2hash_vec = self
            .ts2hash_index
            .iter()
            .filter(|(ts, _)| func_filter(*ts))
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let mut ret = Vec::with_capacity(ts2hash_vec.len());
        for (ts, hash) in ts2hash_vec {
            if let Some(tx) = self.data.get(&hash) {
                if let Some(item) = func_convert(ts, tx) {
                    ret.push(item);
                }
            }
        }
        ret
    }
}

impl Statistics {
    pub(crate) fn new(lifetime_minutes: u32) -> Self {
        let lifetime_dur = Duration::from_secs(u64::from(lifetime_minutes) * 60);
        Self {
            lifetime_dur,
            ..Default::default()
        }
    }

    pub(crate) fn submit_transaction(&mut self, tx: &types::Transaction) {
        ckb_logger::trace!("submit transaction into statistics");
        self.txs.insert(tx);
        self.txs.expire(tx.seen_dt(), self.lifetime_dur);
    }

    pub(crate) fn commit_block(&mut self, block: &types::Block) {
        ckb_logger::trace!("commit block#{} into statistics", block.number());
        self.current_number = block.number();
        self.blocks.insert(block);
        for hash in block.tx_hashes().iter().skip(1) {
            self.txs.remove(hash);
        }
        let block_dt = Duration::from_millis(block.timestamp());
        self.blocks.expire(block_dt, self.lifetime_dur);
    }

    pub(crate) fn reject_transaction(&mut self, tx: &types::RejectedTransaction) {
        ckb_logger::trace!("reject transaction into statistics");
        if tx.is_invalid() {
            self.txs.remove(&tx.hash());
        }
    }

    pub(crate) fn filter_transactions<F, C, T>(&self, func_filter: F, func_convert: C) -> Vec<T>
    where
        F: Fn(Duration) -> bool,
        C: Fn(Duration, &types::Transaction) -> Option<T>,
    {
        self.txs.filter(func_filter, func_convert)
    }

    pub(crate) fn filter_blocks<F, C, T>(&self, func_filter: F, func_convert: C) -> Vec<T>
    where
        F: Fn(Duration) -> bool,
        C: Fn(Duration, &types::Block) -> Option<T>,
    {
        self.blocks.filter(func_filter, func_convert)
    }
}
