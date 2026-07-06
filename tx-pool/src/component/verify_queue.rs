//! Top-level VerifyQueue structure.
#![allow(missing_docs)]
extern crate rustc_hash;
extern crate slab;
use crate::component::flight_tracker::FlightTracker;
use crate::resolved_tx::ResolvedTx;
use ckb_app_config::VerifyOrdering;
use ckb_logger::error;
use ckb_network::PeerIndex;
use ckb_systemtime::unix_time_as_millis;
use ckb_types::core::tx_pool::get_transaction_weight;
use ckb_types::{
    core::{Cycle, FeeRate, TransactionView, tx_pool::Reject},
    packed::ProposalShortId,
};
use ckb_util::shrink_to_fit;
use multi_index_map::MultiIndexMap;
use std::sync::Arc;
use tokio::sync::Notify;

use crate::constants::{DEFAULT_MAX_PIPELINE_QUEUE_TX_SIZE, SHRINK_THRESHOLD};

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

/// Compute an inverted fee rate so that ascending iteration over the index
/// yields descending fee-rate order.  `u64::MAX - fee_rate` is used as the
/// sort key.
fn invert_fee_rate(fee_rate: u64) -> u64 {
    u64::MAX - fee_rate
}

#[derive(MultiIndexMap, Clone)]
struct VerifyEntry {
    /// The transaction id
    #[multi_index(hashed_unique)]
    id: ProposalShortId,
    /// The unix timestamp when entering the Txpool, unit: Millisecond.
    /// Used as the primary sort key in `ArrivalTime` mode.
    #[multi_index(ordered_non_unique)]
    added_time: u64,
    /// Inverted fee rate (`u64::MAX - fee_rate`) so that ascending iteration
    /// yields highest-fee-rate-first ordering.  Used as the primary sort key
    /// in `FeeRate` mode.
    #[multi_index(ordered_non_unique)]
    inverted_fee_rate: u64,
    /// Orders proposal txs before non-proposal txs, preserving arrival order
    /// within each group.  Used for O(1) proposal lookup in `ArrivalTime` mode.
    #[multi_index(ordered_non_unique)]
    priority_order: (VerifyPriority, u64),

    /// whether the tx is a large cycle tx
    #[multi_index(hashed_non_unique)]
    is_large_cycle: bool,
    /// Whether this is a proposal transaction.
    is_proposal_tx: bool,

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
    /// Ordering strategy: arrival time (FIFO) or fee rate.
    ordering: VerifyOrdering,
}

impl VerifyQueue {
    /// Create a new VerifyQueue
    pub(crate) fn new(large_cycle_threshold: u64, ordering: VerifyOrdering) -> Self {
        VerifyQueue {
            inner: MultiIndexVerifyEntryMap::default(),
            ready_rx: Arc::new(Notify::new()),
            total_tx_size: 0,
            large_cycle_threshold,
            flight: FlightTracker::new(),
            ordering,
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
        self.total_tx_size.saturating_add(add_tx_size) >= DEFAULT_MAX_PIPELINE_QUEUE_TX_SIZE
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

    /// Remove multiple txs from the queue from a specified peer.
    /// Returns the short ids of the removed txs.
    pub fn remove_txs_by_peer(&mut self, peer: &PeerIndex) -> Vec<ProposalShortId> {
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

        let removed = ids.clone();
        self.remove_txs(ids.into_iter());
        removed
    }

    /// Returns the first entry in the queue and remove it
    pub fn pop_front(&mut self, only_small_cycle: bool) -> Option<Entry> {
        if let Some(short_id) = self.peek(only_small_cycle) {
            self.remove_tx(&short_id)
        } else {
            None
        }
    }

    /// Returns the first entry in the queue.
    ///
    /// Proposal transactions are always prioritised first (they come from
    /// committed blocks and must be verified to update pool state).
    ///
    /// In `ArrivalTime` mode (default), non-proposal txs are ordered FIFO.
    /// In `FeeRate` mode, non-proposal txs are ordered by descending fee rate.
    pub fn peek(&self, only_small_cycle: bool) -> Option<ProposalShortId> {
        match self.ordering {
            VerifyOrdering::ArrivalTime => self.peek_by_arrival_time(only_small_cycle),
            VerifyOrdering::FeeRate => self.peek_by_fee_rate(only_small_cycle),
        }
    }

    fn peek_by_arrival_time(&self, only_small_cycle: bool) -> Option<ProposalShortId> {
        // `priority_order` sorts proposals before normal txs, with arrival time
        // as the secondary key, so a single iterator gives the correct priority
        // without scanning the whole queue.
        if only_small_cycle {
            self.inner
                .iter_by_priority_order()
                .find(|e| !e.is_large_cycle)
                .map(|e| e.inner.tx().proposal_short_id())
        } else {
            self.inner
                .iter_by_priority_order()
                .next()
                .map(|e| e.inner.tx().proposal_short_id())
        }
    }

    fn peek_by_fee_rate(&self, only_small_cycle: bool) -> Option<ProposalShortId> {
        // Proposals first — within proposals, highest fee rate wins.
        if let Some(proposal_entry) = self
            .inner
            .iter_by_inverted_fee_rate()
            .find(|e| e.is_proposal_tx && !(only_small_cycle && e.is_large_cycle))
        {
            return Some(proposal_entry.inner.tx().proposal_short_id());
        }

        // Non-proposal txs — highest fee rate first.
        let entry = if only_small_cycle {
            self.inner
                .iter_by_inverted_fee_rate()
                .find(|e| !e.is_large_cycle)
        } else {
            self.inner.iter_by_inverted_fee_rate().next()
        };

        entry.map(|e| e.inner.tx().proposal_short_id())
    }

    /// Compute fee rate for a resolved tx.
    ///
    /// For remote txs, declared cycles are used to compute weight.
    /// For local txs (cycles unknown), `large_cycle_threshold` is used as a
    /// conservative estimate so that local txs are not unfairly penalised.
    fn compute_fee_rate(&self, resolved: &ResolvedTx) -> FeeRate {
        let declared_cycles = resolved
            .remote
            .map(|(cycles, _)| cycles)
            .unwrap_or(self.large_cycle_threshold);
        let weight = get_transaction_weight(resolved.tx_size, declared_cycles);
        FeeRate::calculate(resolved.fee, weight)
    }

    /// If the queue did not have this tx present, true is returned.
    /// If the queue did have this tx present, false is returned.
    pub fn add_tx(&mut self, resolved: ResolvedTx) -> Result<bool, Reject> {
        let id = resolved.tx.proposal_short_id();
        if self.contains_key(&id) {
            if resolved.is_proposal_tx {
                // If the existing entry is not yet a proposal tx, promote it
                // in place without the costly remove + shrink + reinsert cycle.
                let mut promoted = false;
                self.inner.modify_by_id(&id, |e| {
                    let now = unix_time_as_millis();
                    if !e.is_proposal_tx {
                        e.is_proposal_tx = true;
                        e.added_time = now;
                        e.priority_order = (VerifyPriority::Proposal, now);
                        promoted = true;
                    } else {
                        // Already a proposal tx — update the timestamp to re-promote.
                        e.added_time = now;
                        e.priority_order.1 = now;
                    }
                });
                return Ok(promoted);
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
        let is_proposal_tx = resolved.is_proposal_tx;
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
        let fee_rate = self.compute_fee_rate(&resolved);
        self.inner.insert(VerifyEntry {
            id: id.clone(),
            added_time,
            inverted_fee_rate: invert_fee_rate(fee_rate.as_u64()),
            priority_order: (priority, added_time),
            inner: Entry { resolved },
            is_large_cycle,
            is_proposal_tx,
        });
        self.flight.insert(
            id.clone(),
            self.inner.get_by_id(&id).expect("just inserted").inner.tx(),
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
