//! First-stage queue for the tx-pool pipeline.
//!
//! Holds raw transactions waiting for the single ordered resolver.

use crate::component::flight_tracker::FlightTracker;
use crate::error::Reject;
use crate::resolved_tx::ResolveJob;
use ckb_util::LinkedHashMap;
use ckb_util::shrink_to_fit;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::Notify;

// 256mb for total_tx_size limit, default max_tx_pool_size is 180mb
const DEFAULT_MAX_RESOLVE_QUEUE_TX_SIZE: usize = 256_000_000;
const SHRINK_THRESHOLD: usize = 100;

/// Queue of raw transactions waiting to be resolved.
pub(crate) struct ResolveQueue {
    /// FIFO queue of resolve jobs.
    inner: VecDeque<ResolveJob>,
    /// Index used for O(1) duplicate checks and removal.
    index: LinkedHashMap<ckb_types::packed::ProposalShortId, ()>,
    /// Subscribe this notify to get notified when an item is added.
    ready_rx: Arc<Notify>,
    /// Total tx size in the queue; new txs are rejected if this would exceed the limit.
    total_tx_size: usize,
    /// Output out-points of txs currently in this queue.
    flight: FlightTracker,
}

impl ResolveQueue {
    /// Create a new resolve queue.
    pub(crate) fn new() -> Self {
        Self {
            inner: VecDeque::new(),
            index: LinkedHashMap::default(),
            ready_rx: Arc::new(Notify::new()),
            total_tx_size: 0,
            flight: FlightTracker::new(),
        }
    }

    /// Returns true if the given tx spends an output produced by another tx
    /// that is currently in this queue.
    pub fn depends_on(&self, tx: &ckb_types::core::TransactionView) -> bool {
        self.flight.depends_on(tx)
    }

    /// Returns true if the queue contains no txs.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Number of jobs in the queue.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    #[cfg(test)]
    pub fn set_total_tx_size_for_test(&mut self, total_tx_size: usize) {
        self.total_tx_size = total_tx_size;
    }

    fn tx_size(job: &ResolveJob) -> usize {
        job.tx.data().serialized_size_in_block()
    }

    /// Returns true if the queue is full.
    pub fn is_full(&self, add_tx_size: usize) -> bool {
        add_tx_size >= DEFAULT_MAX_RESOLVE_QUEUE_TX_SIZE - self.total_tx_size
    }

    /// Returns true if the queue contains a tx with the specified id.
    #[allow(dead_code)]
    pub fn contains_key(&self, id: &ckb_types::packed::ProposalShortId) -> bool {
        self.index.contains_key(id)
    }

    /// Remove a tx from the queue.
    pub fn remove_tx(&mut self, id: &ckb_types::packed::ProposalShortId) -> Option<ResolveJob> {
        if self.index.remove(id).is_some() {
            // VecDeque does not support O(1) removal by id; rebuild the deque.
            // This is acceptable because remove_tx is only called when the queue
            // is explicitly cleared or a tx is cancelled, not on the hot path.
            let mut new_inner = VecDeque::with_capacity(self.inner.len());
            let mut removed = None;
            for job in self.inner.drain(..) {
                if &job.tx.proposal_short_id() == id {
                    let tx_size = Self::tx_size(&job);
                    self.total_tx_size = self.total_tx_size.saturating_sub(tx_size);
                    removed = Some(job);
                } else {
                    new_inner.push_back(job);
                }
            }
            self.inner = new_inner;
            self.shrink_to_fit();
            removed
        } else {
            None
        }
    }

    /// Remove multiple txs from the queue in a single drain-rebuild pass.
    pub fn remove_txs(&mut self, ids: impl Iterator<Item = ckb_types::packed::ProposalShortId>) {
        let to_remove: HashSet<ckb_types::packed::ProposalShortId> = ids.collect();
        if to_remove.is_empty() {
            return;
        }
        for id in &to_remove {
            self.index.remove(id);
            self.flight.remove(id);
        }
        let mut new_inner = VecDeque::with_capacity(self.inner.len());
        for job in self.inner.drain(..) {
            if to_remove.contains(&job.tx.proposal_short_id()) {
                let tx_size = Self::tx_size(&job);
                self.total_tx_size = self.total_tx_size.saturating_sub(tx_size);
            } else {
                new_inner.push_back(job);
            }
        }
        self.inner = new_inner;
        self.shrink_to_fit();
    }

    /// Remove all txs submitted by the given peer.
    pub fn remove_txs_by_peer(&mut self, peer: &ckb_network::PeerIndex) {
        let ids: Vec<_> = self
            .inner
            .iter()
            .filter(|job| job.remote.as_ref().is_some_and(|(_, p)| p == peer))
            .map(|job| job.tx.proposal_short_id())
            .collect();
        self.remove_txs(ids.into_iter());
    }

    /// Returns the first entry in the queue and removes it.
    pub fn pop_front(&mut self) -> Option<ResolveJob> {
        let job = self.inner.pop_front()?;
        let id = job.tx.proposal_short_id();
        self.index.remove(&id);
        self.flight.remove(&id);
        let tx_size = Self::tx_size(&job);
        self.total_tx_size = self.total_tx_size.saturating_sub(tx_size);
        Some(job)
    }

    /// Add a job to the back of the queue.
    ///
    /// Returns `Ok(true)` if the job was newly added, `Ok(false)` if it was a
    /// duplicate, and `Err(Reject::Full)` if the queue is full.
    pub fn add_tx(&mut self, job: ResolveJob) -> Result<bool, Reject> {
        let id = job.tx.proposal_short_id();
        if self.index.contains_key(&id) {
            return Ok(false);
        }
        let tx_size = Self::tx_size(&job);
        if self.is_full(tx_size) {
            return Err(Reject::Full(format!(
                "resolve_queue total_tx_size exceeded, failed to add tx: {:#x}",
                job.tx.hash()
            )));
        }
        let total_tx_size = self.total_tx_size.checked_add(tx_size).ok_or_else(|| {
            Reject::Full(format!(
                "resolve_queue total_tx_size overflowed, failed to add tx: {:#x}",
                job.tx.hash()
            ))
        })?;
        self.index.insert(id.clone(), ());
        self.inner.push_back(job);
        self.flight
            .insert(id, &self.inner.back().expect("just pushed").tx);
        self.total_tx_size = total_tx_size;
        self.ready_rx.notify_one();
        Ok(true)
    }

    /// Subscribe to queue readiness notifications.
    pub fn subscribe(&self) -> Arc<Notify> {
        Arc::clone(&self.ready_rx)
    }

    /// Clears the queue, removing all elements.
    pub fn clear(&mut self) {
        self.inner.clear();
        self.index.clear();
        self.flight.clear();
        self.total_tx_size = 0;
        self.shrink_to_fit();
    }

    fn shrink_to_fit(&mut self) {
        shrink_to_fit!(self.inner, SHRINK_THRESHOLD);
    }
}
