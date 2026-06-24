//! Top-level VerifyQueue structure.
#![allow(missing_docs)]
extern crate rustc_hash;
extern crate slab;
use crate::component::flight_tracker::FlightTracker;
use crate::resolved_tx::ResolvedTx;
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
#[derive(Debug, Clone)]
pub struct Entry {
    /// Resolved transaction ready for verification.
    pub(crate) resolved: ResolvedTx,
}

impl Entry {
    /// The raw transaction.
    pub fn tx(&self) -> &TransactionView {
        &self.resolved.tx
    }

    /// Declared cycles and source peer, if this is a remote transaction.
    pub fn remote(&self) -> Option<(Cycle, PeerIndex)> {
        self.resolved.remote
    }
}

impl PartialEq for Entry {
    fn eq(&self, other: &Entry) -> bool {
        self.tx() == other.tx()
    }
}

#[derive(MultiIndexMap, Clone)]
struct VerifyEntry {
    /// The transaction id
    #[multi_index(hashed_unique)]
    id: ProposalShortId,

    /// whether the tx is a large cycle tx
    #[multi_index(hashed_non_unique)]
    is_large_cycle: bool,
    /// Orders proposal txs before non-proposal txs, preserving arrival order within each group.
    #[multi_index(ordered_non_unique)]
    priority_order: (VerifyPriority, u64),

    /// other sort key
    inner: Entry,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
enum VerifyPriority {
    Proposal,
    Normal,
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
    /// Output out-points of txs currently in this queue.
    flight: FlightTracker,
}

impl VerifyQueue {
    /// Create a new VerifyQueue
    pub(crate) fn new(large_cycle_threshold: u64) -> Self {
        VerifyQueue {
            inner: MultiIndexVerifyEntryMap::default(),
            ready_rx: Arc::new(Notify::new()),
            total_tx_size: 0,
            large_cycle_threshold,
            flight: FlightTracker::new(),
        }
    }

    /// Returns true if the given tx spends an output produced by another tx
    /// that is currently in this queue.
    pub fn depends_on(&self, tx: &TransactionView) -> bool {
        self.flight.depends_on(tx)
    }

    fn recompute_total_tx_size(&self) -> Option<usize> {
        self.inner.iter().try_fold(0usize, |total, (_, entry)| {
            total.checked_add(entry.inner.resolved.tx_size)
        })
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

    /// Returns true if the queue contains a tx with the specified id.
    pub fn get_tx_by_id(&self, id: &ProposalShortId) -> Option<&Entry> {
        self.inner.get_by_id(id).map(|e| &e.inner)
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
        self.flight.remove(id);
        self.inner.remove_by_id(id).map(|e| {
            let tx_size = e.inner.resolved.tx_size;
            if let Some(total_tx_size) = self.total_tx_size.checked_sub(tx_size) {
                self.total_tx_size = total_tx_size;
            } else if let Some(total_tx_size) = self.recompute_total_tx_size() {
                error!(
                    "verify_queue total_tx_size {} underflowed by sub {}, recomputed {}",
                    self.total_tx_size, tx_size, total_tx_size
                );
                self.total_tx_size = total_tx_size;
            } else {
                error!(
                    "verify_queue total_tx_size {} underflowed by sub {}, and recomputing overflowed",
                    self.total_tx_size, tx_size
                );
            }
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

    /// Remove multiple txs from the queue from a specified peer
    pub fn remove_txs_by_peer(&mut self, peer: &PeerIndex) {
        let ids: Vec<_> = self
            .inner
            .iter()
            .filter(|&(_cycle, entry)| {
                entry
                    .inner
                    .resolved
                    .remote
                    .as_ref()
                    .is_some_and(|(_, p)| p == peer)
            })
            .map(|(_cycle, entry)| entry.id.clone())
            .collect();

        self.remove_txs(ids.into_iter());
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
        let first_entry = self.inner.iter_by_priority_order().next();
        if let Some(entry) = first_entry
            && matches!(entry.priority_order.0, VerifyPriority::Proposal)
        {
            return Some(entry.inner.tx().proposal_short_id());
        }

        let entry = if only_small_cycle {
            self.inner
                .iter_by_priority_order()
                .find(|e| !e.is_large_cycle)
        } else {
            first_entry
        };

        entry.map(|e| e.inner.tx().proposal_short_id())
    }

    /// If the queue did not have this tx present, true is returned.
    /// If the queue did have this tx present, false is returned.
    pub fn add_tx(&mut self, resolved: ResolvedTx) -> Result<bool, Reject> {
        let id = resolved.tx.proposal_short_id();
        if self.contains_key(&id) {
            if resolved.is_proposal_tx {
                self.remove_tx(&id);
            } else {
                return Ok(false);
            }
        }
        let tx_size = resolved.tx_size;
        let is_large_cycle = resolved
            .remote
            .map(|(cycles, _)| cycles > self.large_cycle_threshold)
            .unwrap_or(false);
        let added_time = unix_time_as_millis();
        let priority = if is_proposal_tx {
            VerifyPriority::Proposal
        } else {
            VerifyPriority::Normal
        };
        if self.is_full(tx_size) {
            return Err(Reject::Full(format!(
                "verify_queue total_tx_size exceeded, failed to add tx: {:#x}",
                resolved.tx.hash()
            )));
        }
        let total_tx_size = self.total_tx_size.checked_add(tx_size).ok_or_else(|| {
            Reject::Full(format!(
                "verify_queue total_tx_size overflowed, failed to add tx: {:#x}",
                resolved.tx.hash()
            ))
        })?;
        let is_proposal_tx = resolved.is_proposal_tx;
        self.inner.insert(VerifyEntry {
            id: id.clone(),
            inner: Entry { resolved },
            is_large_cycle,
            priority_order: (priority, added_time),
        });
        self.flight.insert(
            id.clone(),
            &self.inner.get_by_id(&id).expect("just inserted").inner.tx(),
        );
        self.total_tx_size = total_tx_size;
        self.ready_rx.notify_one();
        Ok(true)
    }

    /// When OnlySmallCycleTx Worker is wakeup, but found the tx is large cycle tx, notify other workers.
    pub fn re_notify(&self) {
        self.ready_rx.notify_waiters();
    }

    /// Clears the map, removing all elements.
    pub fn clear(&mut self) {
        self.inner.clear();
        self.flight.clear();
        self.total_tx_size = 0;
        self.shrink_to_fit();
    }
}
