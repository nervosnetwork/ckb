//! Second-stage ordered queue for transactions that could not be pre-resolved.
//!
//! When the concurrent pre-resolver meets a transaction whose inputs are not yet
//! available (e.g. it depends on another transaction still in flight), the job
//! is moved here.  A single ordered resolver then processes them sequentially,
//! which keeps the ordering guarantees for dependent transactions.

use crate::resolved_tx::ResolveJob;
use ckb_types::packed::ProposalShortId;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::Notify;

/// Ordered queue of raw transactions waiting for the single ordered resolver.
pub(crate) struct OrderedResolveQueue {
    /// FIFO queue of resolve jobs.
    inner: VecDeque<ResolveJob>,
    /// O(1) membership index for `contains_key` and fast `remove_tx` lookup.
    index: HashSet<ProposalShortId>,
    /// Subscribe this notify to get notified when an item is added.
    ready_rx: Arc<Notify>,
}

impl OrderedResolveQueue {
    /// Create a new ordered resolve queue.
    pub(crate) fn new() -> Self {
        Self {
            inner: VecDeque::new(),
            index: HashSet::new(),
            ready_rx: Arc::new(Notify::new()),
        }
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

    /// Returns true if the queue contains a tx with the specified id.
    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.index.contains(id)
    }

    /// Returns the first entry in the queue and removes it.
    pub fn pop_front(&mut self) -> Option<ResolveJob> {
        let job = self.inner.pop_front()?;
        self.index.remove(&job.tx.proposal_short_id());
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
        Some(self.inner.remove(pos).expect("position exists"))
    }

    /// Add a job to the back of the queue.
    pub fn add_tx(&mut self, job: ResolveJob) {
        self.index.insert(job.tx.proposal_short_id());
        self.inner.push_back(job);
        self.ready_rx.notify_one();
    }

    /// Remove all jobs submitted by the given peer.
    pub fn remove_txs_by_peer(&mut self, peer: &ckb_network::PeerIndex) {
        self.inner.retain(|job| {
            if job.remote.as_ref().is_some_and(|(_, p)| p == peer) {
                self.index.remove(&job.tx.proposal_short_id());
                false
            } else {
                true
            }
        });
    }

    /// Subscribe to queue readiness notifications.
    pub fn subscribe(&self) -> Arc<Notify> {
        Arc::clone(&self.ready_rx)
    }

    /// Clears the queue, removing all elements.
    pub fn clear(&mut self) {
        self.inner.clear();
        self.index.clear();
    }
}
