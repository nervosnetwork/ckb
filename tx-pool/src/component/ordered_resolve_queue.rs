//! Ordered queue for transactions that could not be resolved at entry.
//!
//! Transactions whose inputs are not yet available (e.g. they depend on another
//! transaction still in flight) are placed here.  A single ordered resolver
//! retries them in arrival order, which keeps orphan-pool churn low for
//! dependent transactions.

use crate::component::flight_tracker::FlightTracker;
use crate::error::Reject;
use crate::resolved_tx::ResolveJob;
use ckb_logger::error;
use ckb_network::PeerIndex;
use ckb_types::core::TransactionView;
use ckb_types::packed::ProposalShortId;
use ckb_util::shrink_to_fit;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::Notify;

// 256mb for total_tx_size limit, default max_tx_pool_size is 180mb
const DEFAULT_MAX_ORDERED_RESOLVE_QUEUE_TX_SIZE: usize = 256_000_000;
const SHRINK_THRESHOLD: usize = 100;

/// Ordered queue of raw transactions waiting for the ordered resolver.
pub(crate) struct OrderedResolveQueue {
    /// FIFO queue of resolve jobs.
    inner: VecDeque<ResolveJob>,
    /// O(1) membership index for `contains_key` and fast `remove_tx` lookup.
    index: HashSet<ProposalShortId>,
    /// Subscribe this notify to get notified when an item is added.
    ready_rx: Arc<Notify>,
    /// Total tx size in the queue; new txs are rejected if this would exceed the limit.
    total_tx_size: usize,
    /// Output out-points of txs currently in this queue.
    flight: FlightTracker,
}

impl OrderedResolveQueue {
    /// Create a new ordered resolve queue.
    pub(crate) fn new() -> Self {
        Self {
            inner: VecDeque::new(),
            index: HashSet::new(),
            ready_rx: Arc::new(Notify::new()),
            total_tx_size: 0,
            flight: FlightTracker::new(),
        }
    }

    /// Returns true if the given tx spends an output produced by another tx
    /// that is currently in this queue.
    pub fn depends_on(&self, tx: &TransactionView) -> bool {
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
        self.total_tx_size.saturating_add(add_tx_size) >= DEFAULT_MAX_ORDERED_RESOLVE_QUEUE_TX_SIZE
    }

    /// Returns true if the queue contains a tx with the specified id.
    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.index.contains(id)
    }

    /// Returns the raw transaction for the given id, if present.
    pub fn get_tx(&self, id: &ProposalShortId) -> Option<&TransactionView> {
        self.inner
            .iter()
            .find(|job| &job.tx.proposal_short_id() == id)
            .map(|job| &job.tx)
    }

    fn shrink_to_fit(&mut self) {
        shrink_to_fit!(self.inner, SHRINK_THRESHOLD);
    }

    /// Returns the first entry in the queue and removes it.
    pub fn pop_front(&mut self) -> Option<ResolveJob> {
        let job = self.inner.pop_front()?;
        let id = job.tx.proposal_short_id();
        self.index.remove(&id);
        self.flight.remove(&id);
        if let Some(total) = self.total_tx_size.checked_sub(Self::tx_size(&job)) {
            self.total_tx_size = total;
        } else {
            error!(
                "ordered_resolve_queue total_tx_size {} underflowed by sub {} in pop_front",
                self.total_tx_size,
                Self::tx_size(&job)
            );
            self.total_tx_size = 0;
        }
        Some(job)
    }

    /// Remove a tx from the queue by its short id.
    pub fn remove_tx(&mut self, id: &ProposalShortId) -> Option<ResolveJob> {
        if !self.index.remove(id) {
            return None;
        }
        let pos = self
            .inner
            .iter()
            .position(|job| &job.tx.proposal_short_id() == id)
            .expect("index says id exists");
        let job = self.inner.remove(pos).expect("position exists");
        self.flight.remove(id);
        if let Some(total) = self.total_tx_size.checked_sub(Self::tx_size(&job)) {
            self.total_tx_size = total;
        } else {
            error!(
                "ordered_resolve_queue total_tx_size {} underflowed by sub {} in remove_tx",
                self.total_tx_size,
                Self::tx_size(&job)
            );
            self.total_tx_size = 0;
        }
        self.shrink_to_fit();
        Some(job)
    }

    /// Remove all jobs submitted by the given peer.
    pub fn remove_txs_by_peer(&mut self, peer: &PeerIndex) {
        self.inner.retain(|job| {
            if job.remote.as_ref().is_some_and(|(_, p)| p == peer) {
                let id = job.tx.proposal_short_id();
                self.index.remove(&id);
                self.flight.remove(&id);
                if let Some(total) = self.total_tx_size.checked_sub(Self::tx_size(job)) {
                    self.total_tx_size = total;
                } else {
                    error!(
                        "ordered_resolve_queue total_tx_size {} underflowed by sub {} in remove_txs_by_peer",
                        self.total_tx_size,
                        Self::tx_size(job)
                    );
                    self.total_tx_size = 0;
                }
                false
            } else {
                true
            }
        });
        self.shrink_to_fit();
    }

    /// Add a job to the back of the queue.
    ///
    /// Returns `Ok(true)` if the job was newly added, `Ok(false)` if it was a
    /// duplicate, and `Err(Reject::Full)` if the queue is full.
    pub fn add_tx(&mut self, job: ResolveJob) -> Result<bool, Reject> {
        let id = job.tx.proposal_short_id();
        if self.index.contains(&id) {
            return Ok(false);
        }
        let tx_size = Self::tx_size(&job);
        let tx_hash = job.tx.hash();
        if self.is_full(tx_size) {
            return Err(Reject::Full(format!(
                "ordered_resolve_queue total_tx_size exceeded, failed to add tx: {tx_hash:#x}",
            )));
        }
        self.total_tx_size = self.total_tx_size.checked_add(tx_size).ok_or_else(|| {
            Reject::Full(format!(
                "ordered_resolve_queue total_tx_size overflowed, failed to add tx: {tx_hash:#x}",
            ))
        })?;
        self.index.insert(id.clone());
        self.inner.push_back(job);
        self.flight
            .insert(id, &self.inner.back().expect("just pushed").tx);
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
}
