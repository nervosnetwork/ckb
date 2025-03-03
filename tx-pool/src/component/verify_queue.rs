//! Top-level VerifyQueue structure.
#![allow(missing_docs)]
extern crate rustc_hash;
extern crate slab;
use ckb_logger::error;
use ckb_network::PeerIndex;
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{
    core::{Cycle, TransactionView, tx_pool::Reject},
    packed::ProposalShortId,
};
use ckb_util::shrink_to_fit;
use multi_index_map::MultiIndexMap;
use std::sync::Arc;
use tokio::sync::Notify;

// 256mb for total_tx_size limit, default max_tx_pool_size is 180mb
const DEFAULT_MAX_VERIFY_QUEUE_TX_SIZE: usize = 256_000_000;
const SHRINK_THRESHOLD: usize = 100;

/// The verify queue Entry to verify.
#[derive(Debug, Clone, Eq)]
pub struct Entry {
    pub(crate) tx: TransactionView,
    pub(crate) remote: Option<(Cycle, PeerIndex)>,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Entry) -> bool {
        self.tx == other.tx
    }
}

#[derive(MultiIndexMap, Clone)]
struct VerifyEntry {
    /// The transaction id
    #[multi_index(hashed_unique)]
    id: ProposalShortId,
    /// The unix timestamp when entering the Txpool, unit: Millisecond
    /// This field is used to sort the txs in the queue
    /// We may add more other sort keys in the future
    #[multi_index(ordered_non_unique)]
    added_time: u64,

    /// whether the tx is a large cycle tx
    #[multi_index(hashed_non_unique)]
    is_large_cycle: bool,

    /// other sort key
    inner: Entry,
}

/// The verify queue is a priority queue of transactions to verify.
pub(crate) struct VerifyQueue {
    /// inner tx entry
    inner: MultiIndexVerifyEntryMap,
    /// subscribe this notify to get be notified when there is item in the queue
    ready_rx: Arc<Notify>,
    /// total tx size in the queue, will reject new transaction if exceed the limit
    total_tx_size: usize,
    /// large cycle threshold, from `pool_config.max_tx_verify_cycles`
    large_cycle_threshold: u64,
}

impl VerifyQueue {
    /// Create a new VerifyQueue
    pub(crate) fn new(large_cycle_threshold: u64) -> Self {
        VerifyQueue {
            inner: MultiIndexVerifyEntryMap::default(),
            ready_rx: Arc::new(Notify::new()),
            total_tx_size: 0,
            large_cycle_threshold,
        }
    }

    /// Returns true if the queue contains no txs.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    #[cfg(test)]
    pub fn total_tx_size(&self) -> usize {
        self.total_tx_size
    }

    /// Returns true if the queue is full.
    pub fn is_full(&self, add_tx_size: usize) -> bool {
        add_tx_size >= DEFAULT_MAX_VERIFY_QUEUE_TX_SIZE - self.total_tx_size
    }

    /// Returns true if the queue contains a tx with the specified id.
    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.inner.get_by_id(id).is_some()
    }

    /// Shrink the capacity of the queue as much as possible.
    pub fn shrink_to_fit(&mut self) {
        shrink_to_fit!(self.inner, SHRINK_THRESHOLD);
    }

    /// get a queue_rx to subscribe the txs count in the queue
    pub fn subscribe(&self) -> Arc<Notify> {
        Arc::clone(&self.ready_rx)
    }

    /// Remove a tx from the queue
    pub fn remove_tx(&mut self, id: &ProposalShortId) -> Option<Entry> {
        self.inner.remove_by_id(id).map(|e| {
            let tx_size = e.inner.tx.data().serialized_size_in_block();
            self.total_tx_size = self.total_tx_size.checked_sub(tx_size).unwrap_or_else(|| {
                error!(
                    "verify_queue total_tx_size {} overflown by sub {}",
                    self.total_tx_size, tx_size
                );
                0
            });
            self.shrink_to_fit();
            e.inner
        })
    }

    /// Remove multiple txs from the queue
    pub fn remove_txs(&mut self, ids: impl Iterator<Item = ProposalShortId>) {
        for id in ids {
            self.remove_tx(&id);
        }
    }

    /// Returns the first entry in the queue and remove it
    pub fn pop_front(&mut self, only_small_cycle: bool) -> Option<Entry> {
        if let Some(short_id) = self.peek(only_small_cycle) {
            self.remove_tx(&short_id)
        } else {
            None
        }
    }

    /// Returns the first entry in the queue
    pub fn peek(&self, only_small_cycle: bool) -> Option<ProposalShortId> {
        if only_small_cycle {
            self.inner.iter_by_added_time().find(|e| !e.is_large_cycle)
        } else {
            self.inner.iter_by_added_time().next()
        }
        .map(|entry| entry.inner.tx.proposal_short_id())
    }

    /// If the queue did not have this tx present, true is returned.
    /// If the queue did have this tx present, false is returned.
    pub fn add_tx(
        &mut self,
        tx: TransactionView,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Result<bool, Reject> {
        if self.contains_key(&tx.proposal_short_id()) {
            return Ok(false);
        }
        let tx_size = tx.data().serialized_size_in_block();
        let is_large_cycle = remote
            .map(|(cycles, _)| cycles > self.large_cycle_threshold)
            .unwrap_or(false);
        if self.is_full(tx_size) {
            return Err(Reject::Full(format!(
                "verify_queue total_tx_size exceeded, failed to add tx: {:#x}",
                tx.hash()
            )));
        }
        self.inner.insert(VerifyEntry {
            id: tx.proposal_short_id(),
            added_time: unix_time_as_millis(),
            inner: Entry { tx, remote },
            is_large_cycle,
        });
        self.total_tx_size = self.total_tx_size.checked_add(tx_size).unwrap_or_else(|| {
            error!(
                "verify_queue total_tx_size {} overflown by add {}",
                self.total_tx_size, tx_size
            );
            self.total_tx_size
        });
        self.ready_rx.notify_one();
        Ok(true)
    }

    /// When OnlySmallCycleTx Worker is wakeup, but found the tx is large cycle tx, notify other workers.
    pub fn re_notify(&self) {
        self.ready_rx.notify_one();
    }

    /// Clears the map, removing all elements.
    pub fn clear(&mut self) {
        self.inner.clear();
        self.total_tx_size = 0;
        self.shrink_to_fit();
    }
}
