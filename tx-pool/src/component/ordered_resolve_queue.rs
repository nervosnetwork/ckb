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
use ckb_types::packed::{OutPoint, ProposalShortId};
use ckb_util::shrink_to_fit;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::Notify;

// 256mb for total_tx_size limit, default max_tx_pool_size is 180mb
const DEFAULT_MAX_ORDERED_RESOLVE_QUEUE_TX_SIZE: usize = 256_000_000;
const SHRINK_THRESHOLD: usize = 100;

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
    total_tx_size: usize,
    /// Output out-points of txs currently in this queue.
    flight: FlightTracker,
    /// Reverse index: output outpoint → set of job ids that consume it as an
    /// input.  When a parent transaction enters the pool, we use this to
    /// immediately find and re-enqueue its dependent children, bypassing the
    /// FIFO scan.
    output_dependents: HashMap<OutPoint, HashSet<ProposalShortId>>,
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
            total_tx_size: 0,
            flight: FlightTracker::new(),
            output_dependents: HashMap::new(),
        }
    }

    /// Returns true if the given tx spends an output produced by another tx
    /// that is currently in this queue.
    pub fn depends_on(&self, tx: &TransactionView) -> bool {
        self.flight.depends_on(tx)
    }

    /// Returns true if the queue contains no live (non-tombstoned) txs.
    pub fn is_empty(&self) -> bool {
        self.live_count == 0
    }

    /// Number of live jobs in the queue (excludes tombstoned entries).
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.live_count
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

    /// Returns true if the queue contains a live tx with the specified id.
    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.lookup.contains_key(id)
    }

    /// Returns the raw transaction for the given id, if present.  O(1).
    pub fn get_tx(&self, id: &ProposalShortId) -> Option<&TransactionView> {
        self.lookup.get(id).map(|job| &job.tx)
    }

    fn shrink_to_fit(&mut self) {
        shrink_to_fit!(self.inner, SHRINK_THRESHOLD);
        shrink_to_fit!(self.output_dependents, SHRINK_THRESHOLD);
    }

    /// Remove the output_dependents reverse-index entries for a job.
    fn remove_dependents_for(&mut self, id: &ProposalShortId, inputs: &[OutPoint]) {
        for outpoint in inputs {
            if let Some(set) = self.output_dependents.get_mut(outpoint) {
                set.remove(id);
                if set.is_empty() {
                    self.output_dependents.remove(outpoint);
                }
            }
        }
    }

    /// Internal removal: lookup → tombstone → clean up all indexes.
    ///
    /// This is the single source of truth for removing a job from the queue.
    /// All public removal methods (`pop_front`, `remove_tx`,
    /// `remove_txs_by_peer`, `drain_dependents`) delegate here to ensure
    /// every index stays in sync.
    fn remove_job_internal(&mut self, id: &ProposalShortId, context: &str) -> Option<ResolveJob> {
        let job = self.lookup.remove(id)?;
        self.removed.insert(id.clone());
        self.live_count -= 1;
        let inputs: Vec<OutPoint> = job.tx.input_pts_iter().collect();
        self.remove_dependents_for(id, &inputs);
        self.flight.remove(id);
        if let Some(total) = self.total_tx_size.checked_sub(Self::tx_size(&job)) {
            self.total_tx_size = total;
        } else {
            error!(
                "ordered_resolve_queue total_tx_size {} underflowed by sub {} in {}",
                self.total_tx_size,
                Self::tx_size(&job),
                context,
            );
            self.total_tx_size = 0;
        }
        Some(job)
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
        self.remove_job_internal(&id, "pop_front")
    }

    /// Remove a tx from the queue by its short id.  O(1) — marks the entry
    /// as tombstoned; the `VecDeque` slot is reclaimed lazily by `pop_front`.
    pub fn remove_tx(&mut self, id: &ProposalShortId) -> Option<ResolveJob> {
        let job = self.remove_job_internal(id, "remove_tx")?;
        self.drain_tombstones();
        self.shrink_to_fit();
        Some(job)
    }

    /// Remove all jobs submitted by the given peer.
    pub fn remove_txs_by_peer(&mut self, peer: &PeerIndex) {
        // First pass: identify which ids to remove (iterate inner for FIFO order).
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
            self.remove_job_internal(id, "remove_txs_by_peer");
        }

        self.drain_tombstones();
        self.shrink_to_fit();
    }

    /// Add a job to the back of the queue.
    ///
    /// Returns `Ok(true)` if the job was newly added, `Ok(false)` if it was a
    /// duplicate, and `Err(Reject::Full)` if the queue is full.
    pub fn add_tx(&mut self, job: ResolveJob) -> Result<bool, Reject> {
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
        self.total_tx_size = self.total_tx_size.checked_add(tx_size).ok_or_else(|| {
            Reject::Full(format!(
                "ordered_resolve_queue total_tx_size overflowed, failed to add tx: {tx_hash:#x}",
            ))
        })?;

        // Register reverse index: for each input this job consumes, record that
        // this job is waiting for the producer of that outpoint.
        for outpoint in job.tx.input_pts_iter() {
            self.output_dependents
                .entry(outpoint.clone())
                .or_default()
                .insert(id.clone());
        }

        self.flight.insert(id.clone(), &job.tx);
        self.lookup.insert(id.clone(), job);
        self.inner.push_back(id);
        self.live_count += 1;
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
        self.lookup.clear();
        self.removed.clear();
        self.output_dependents.clear();
        self.live_count = 0;
        self.flight.clear();
        self.total_tx_size = 0;
        self.shrink_to_fit();
    }

    /// Extract jobs that are waiting for the given output outpoints,
    /// preserving their original FIFO order.
    ///
    /// When a parent transaction enters the pool, its output outpoints become
    /// available.  This method finds all queued jobs that consume those outputs
    /// as inputs, removes them from the queue, and returns them so the caller
    /// can push them directly to the resolver (bypassing the FIFO scan).
    ///
    /// The returned jobs are in the same relative order as they appeared in
    /// the queue, which matters when multiple children compete for the same
    /// subsequent cell.
    pub fn drain_dependents(&mut self, outputs: &[OutPoint]) -> Vec<ResolveJob> {
        // Collect the set of ids that depend on the given outputs.
        let mut dependent_ids: HashSet<ProposalShortId> = HashSet::new();
        for outpoint in outputs {
            if let Some(ids) = self.output_dependents.remove(outpoint) {
                dependent_ids.extend(ids);
            }
        }

        if dependent_ids.is_empty() {
            return Vec::new();
        }

        // Collect matching ids in FIFO order by iterating inner (VecDeque).
        // This preserves the original queue ordering, which matters when
        // multiple children compete for the same subsequent cell.
        let ordered_ids: Vec<ProposalShortId> = self
            .inner
            .iter()
            .filter(|id| dependent_ids.contains(id))
            .cloned()
            .collect();

        let mut jobs = Vec::with_capacity(ordered_ids.len());
        for id in &ordered_ids {
            if let Some(job) = self.remove_job_internal(id, "drain_dependents") {
                jobs.push(job);
            }
        }

        self.drain_tombstones();
        jobs
    }
}
