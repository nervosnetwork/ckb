//! Ordered queue for transactions that could not be resolved at entry.
//!
//! Transactions whose inputs are not yet available (e.g. they depend on another
//! transaction still in flight) are placed here.  A single ordered resolver
//! retries them in arrival order, which keeps orphan-pool churn low for
//! dependent transactions.

use crate::component::flight_tracker::FlightTracker;
use crate::component::pipeline_queue::PipelineQueue;
use crate::component::saturating_counter::SaturatingCounter;
use crate::error::Reject;
use crate::resolved_tx::ResolveJob;
use ckb_network::PeerIndex;
use ckb_types::core::TransactionView;
use ckb_types::packed::ProposalShortId;
use ckb_util::shrink_to_fit;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::Notify;

use crate::constants::SHRINK_THRESHOLD;

/// Ordered queue of raw transactions waiting for the ordered resolver.
///
/// Uses a `VecDeque<ProposalShortId>` for FIFO ordering and a
/// `HashMap<ProposalShortId, ResolveJob>` for O(1) lookups.  Removals are
/// lazy (tombstoned) so that the `VecDeque` is never shifted; tombstones
/// are drained in `pop_front`.
pub(crate) struct OrderedResolveQueue {
    /// FIFO queue of short ids (ordering only; full data lives in `lookup`).
    inner: VecDeque<ProposalShortId>,
    /// O(1) lookup of resolve jobs by short id.
    lookup: HashMap<ProposalShortId, ResolveJob>,
    /// Tombstoned ids that have been logically removed but still occupy a
    /// slot in `inner`; drained lazily by `pop_front`.
    removed: HashSet<ProposalShortId>,
    /// Number of live (non-tombstoned) entries.  Kept in sync with
    /// `lookup.len()` so that `len()` / `is_empty()` are accurate without
    /// counting tombstones in `inner`.
    live_count: usize,
    /// Subscribe this notify to get notified when an item is added.
    ready_rx: Arc<Notify>,
    /// Total tx size in the queue; new txs are rejected if this would exceed the limit.
    total_tx_size: SaturatingCounter<usize>,
    /// Output out-points of txs currently in this queue.
    flight: FlightTracker,
}

impl OrderedResolveQueue {
    /// Create a new ordered resolve queue.
    pub(crate) fn new() -> Self {
        Self {
            inner: VecDeque::new(),
            lookup: HashMap::new(),
            removed: HashSet::new(),
            live_count: 0,
            ready_rx: Arc::new(Notify::new()),
            total_tx_size: SaturatingCounter::new(0),
            flight: FlightTracker::new(),
        }
    }

    /// Number of live jobs in the queue (excludes tombstoned entries).
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.live_count
    }

    #[cfg(test)]
    pub fn set_total_tx_size_for_test(&mut self, total_tx_size: usize) {
        self.total_tx_size.set(total_tx_size);
    }

    fn tx_size(job: &ResolveJob) -> usize {
        job.tx.data().serialized_size_in_block()
    }

    /// Returns the raw transaction for the given id, if present.  O(1).
    pub fn get_tx(&self, id: &ProposalShortId) -> Option<&TransactionView> {
        self.lookup.get(id).map(|job| &job.tx)
    }

    fn shrink_to_fit(&mut self) {
        shrink_to_fit!(self.inner, SHRINK_THRESHOLD);
    }

    /// Drain tombstoned entries from the front of the FIFO queue.
    fn drain_tombstones(&mut self) {
        while let Some(front_id) = self.inner.front() {
            if self.removed.remove(front_id) {
                self.inner.pop_front();
            } else {
                break;
            }
        }
    }

    /// Returns the first live entry in the queue and removes it.
    pub fn pop_front(&mut self) -> Option<ResolveJob> {
        self.drain_tombstones();
        let id = self.inner.pop_front()?;
        let job = self
            .lookup
            .remove(&id)
            .expect("lookup contains id from inner");
        self.live_count -= 1;
        self.flight.remove(&id);
        self.total_tx_size.sub_or_zero(
            Self::tx_size(&job),
            "ordered_resolve_queue total_tx_size",
            "pop_front",
        );
        Some(job)
    }
}

impl PipelineQueue for OrderedResolveQueue {
    type Tx = ResolveJob;

    fn total_tx_size(&self) -> &SaturatingCounter<usize> {
        &self.total_tx_size
    }

    fn flight(&self) -> &FlightTracker {
        &self.flight
    }

    fn ready_rx(&self) -> &Arc<Notify> {
        &self.ready_rx
    }

    fn is_empty(&self) -> bool {
        self.live_count == 0
    }

    fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.lookup.contains_key(id)
    }

    fn remove_tx(&mut self, id: &ProposalShortId) -> Option<ResolveJob> {
        let job = self.lookup.remove(id)?;
        self.removed.insert(id.clone());
        self.live_count -= 1;
        self.flight.remove(id);
        self.total_tx_size.sub_or_zero(
            Self::tx_size(&job),
            "ordered_resolve_queue total_tx_size",
            "remove_tx",
        );
        // Attempt to reclaim the front slot if this id happens to be there.
        self.drain_tombstones();
        self.shrink_to_fit();
        Some(job)
    }

    fn remove_txs_by_peer(&mut self, peer: &PeerIndex) -> Vec<ProposalShortId> {
        // First pass: identify which ids to remove.
        let to_remove: Vec<ProposalShortId> = self
            .inner
            .iter()
            .filter(|id| {
                self.lookup
                    .get(*id)
                    .is_some_and(|job| job.remote.as_ref().is_some_and(|(_, p)| p == peer))
            })
            .cloned()
            .collect();

        for id in &to_remove {
            if let Some(job) = self.lookup.remove(id) {
                self.removed.insert(id.clone());
                self.live_count -= 1;
                self.flight.remove(id);
                self.total_tx_size.sub_or_zero(
                    Self::tx_size(&job),
                    "ordered_resolve_queue total_tx_size",
                    "remove_txs_by_peer",
                );
            }
        }

        self.drain_tombstones();
        self.shrink_to_fit();
        to_remove
    }

    fn add_tx(&mut self, job: ResolveJob) -> Result<bool, Reject> {
        let id = job.tx.proposal_short_id();
        if self.lookup.contains_key(&id) {
            return Ok(false);
        }
        let tx_size = Self::tx_size(&job);
        let tx_hash = job.tx.hash();
        if self.is_full(tx_size) {
            return Err(Reject::Full(format!(
                "ordered_resolve_queue total_tx_size exceeded, failed to add tx: {tx_hash:#x}",
            )));
        }
        self.total_tx_size.set(
            self.total_tx_size
                .get()
                .checked_add(tx_size)
                .ok_or_else(|| {
                    Reject::Full(format!(
                        "ordered_resolve_queue total_tx_size overflowed, failed to add tx: {tx_hash:#x}",
                    ))
                })?,
        );
        self.flight.insert(id.clone(), &job.tx);
        self.lookup.insert(id.clone(), job);
        self.inner.push_back(id);
        self.live_count += 1;
        self.ready_rx.notify_one();
        Ok(true)
    }

    fn clear(&mut self) {
        self.inner.clear();
        self.lookup.clear();
        self.removed.clear();
        self.live_count = 0;
        self.flight.clear();
        self.total_tx_size.set(0);
        self.shrink_to_fit();
    }
}
