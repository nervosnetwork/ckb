//! Ordered queue for transactions that could not be resolved at entry.
//!
//! Transactions whose inputs are not yet available (e.g. they depend on another
//! transaction still in flight) are placed here.  A single ordered resolver
//! retries them in arrival order, which keeps orphan-pool churn low for
//! dependent transactions.

use crate::component::active_set::ActiveSet;
use crate::component::flight_tracker::FlightTracker;
use crate::component::pipeline_queue::PipelineQueue;
use crate::component::saturating_counter::SaturatingCounter;
use crate::error::Reject;
use crate::resolved_tx::ResolveJob;
use ckb_network::PeerIndex;
use ckb_types::core::TransactionView;
use ckb_types::packed::ProposalShortId;
use ckb_util::shrink_to_fit;
use std::collections::binary_heap::BinaryHeap;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Notify;

use crate::constants::SHRINK_THRESHOLD;
use ckb_logger::debug;

/// Ordered queue of raw transactions waiting for the ordered resolver.
///
/// Uses a `VecDeque<ProposalShortId>` for FIFO ordering and a
/// `HashMap<ProposalShortId, ResolveJob>` for O(1) lookups.  Removals are
/// lazy (tombstoned) so that the `VecDeque` is never shifted; tombstones
/// are drained in `pop_front`.
///
/// Jobs that must not be retried yet (local orphans whose parents are still
/// in flight) live in the `delayed` min-heap instead of the FIFO: they
/// become poppable only once their deadline passes, and stay visible to
/// `lookup`-based operations (`contains_key`, `get_tx`, `remove_tx`) the
/// whole time — unlike the previous spawn-sleep-re-enqueue model, in which
/// a delayed job temporarily existed only inside a detached task.
pub(crate) struct OrderedResolveQueue {
    /// FIFO queue of short ids (ordering only; full data lives in `lookup`).
    inner: VecDeque<ProposalShortId>,
    /// O(1) lookup of resolve jobs by short id, covering both FIFO and
    /// delayed jobs.
    lookup: HashMap<ProposalShortId, LookupEntry>,
    /// Tombstoned ids that have been logically removed but still occupy a
    /// slot in `inner`; drained lazily by `pop_front`.
    removed: HashSet<ProposalShortId>,
    /// Delayed jobs by their retry deadline (earliest first), each tagged
    /// with the generation of the lookup entry it was pushed for. Entries
    /// whose generation no longer matches the lookup entry are stale (the
    /// job was removed and possibly re-added since) and discarded lazily on
    /// pop.
    delayed: BinaryHeap<std::cmp::Reverse<(Instant, u64, ProposalShortId)>>,
    /// Monotonic counter tagging every inserted job; see `delayed`.
    next_generation: u64,
    /// Number of live (non-tombstoned) entries, FIFO and delayed combined.
    /// Kept in sync with `lookup.len()` so that `len()` / `is_empty()` are
    /// accurate without counting tombstones in `inner`.
    live_count: usize,
    /// Jobs that have been popped by the resolver but have not reached a
    /// terminal state yet; see [`OrderedResolveQueue::contains_or_active`].
    active: ActiveSet<TransactionView>,
    /// Subscribe this notify to get notified when an item is added.
    ready_rx: Arc<Notify>,
    /// Total tx size in the queue; new txs are rejected if this would exceed the limit.
    total_tx_size: SaturatingCounter<usize>,
    /// Output out-points of txs currently in this queue.
    flight: FlightTracker,
}

/// A live resolve job plus the generation it was inserted with. Delayed
/// heap entries carry the same generation so a stale entry (pushed before a
/// remove/re-add of the same id) can be told apart from the current job.
struct LookupEntry {
    job: ResolveJob,
    generation: u64,
}

impl OrderedResolveQueue {
    /// Create a new ordered resolve queue.
    pub(crate) fn new() -> Self {
        Self {
            inner: VecDeque::new(),
            lookup: HashMap::new(),
            removed: HashSet::new(),
            delayed: BinaryHeap::new(),
            next_generation: 0,
            live_count: 0,
            active: ActiveSet::default(),
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
        self.lookup.get(id).map(|entry| &entry.job.tx)
    }

    /// Returns true if the given proposal id is either queued (FIFO or
    /// delayed) or currently being processed by the resolver (popped but not
    /// yet terminal).
    ///
    /// Introduced for the local-orphan flight check
    /// (`all_missing_parents_in_flight`): a parent transaction must count as
    /// "in flight" for the whole time it is inside the pipeline, including
    /// the window between `pop_front` and the end of resolution, otherwise
    /// the orphan's retry budget is burned while its parent is mid-flight.
    /// The read-only pipeline query paths use it for the same visibility.
    ///
    /// Deliberately *not* used for duplicate detection (`contains_key`): if
    /// the resolver panics between `pop_front` and `finish`, the id stays in
    /// `active` until the queue is cleared. That only makes orphans wait out
    /// their bounded retry budget; it must never block a genuine
    /// resubmission.
    pub fn contains_or_active(&self, id: &ProposalShortId) -> bool {
        self.lookup.contains_key(id) || self.active.contains(id)
    }

    /// Returns the transaction currently being processed by the resolver
    /// (popped but not yet finished), if any.
    pub fn get_active_tx(&self, id: &ProposalShortId) -> Option<TransactionView> {
        self.active.get(id).cloned()
    }

    /// Mark a previously popped job as terminally processed.
    ///
    /// The resolver must call this exactly once per popped job, after the
    /// job has landed in its terminal state (forwarded to the verify queue,
    /// re-enqueued, or rejected). Re-enqueueing first and calling `finish`
    /// second keeps the id continuously visible to `contains_or_active`.
    pub fn finish(&mut self, id: &ProposalShortId) {
        self.active.finish(id);
    }

    fn shrink_to_fit(&mut self) {
        shrink_to_fit!(self.inner, SHRINK_THRESHOLD);
        shrink_to_fit!(self.delayed, SHRINK_THRESHOLD);
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

    /// Remove a live job by id and apply all accounting (count, flight,
    /// size). Returns the job if it was present.
    fn remove_live(&mut self, id: &ProposalShortId, op: &'static str) -> Option<ResolveJob> {
        let entry = self.lookup.remove(id)?;
        self.live_count -= 1;
        self.flight.remove(id);
        let tx_size = Self::tx_size(&entry.job);
        // On underflow (an accounting bug elsewhere), restore the true
        // total from what remains instead of clamping to zero, which
        // would silently disable the size budget.
        let recompute = self
            .total_tx_size
            .get()
            .checked_sub(tx_size)
            .is_none()
            .then(|| {
                self.lookup.values().try_fold(0usize, |acc, entry| {
                    acc.checked_add(Self::tx_size(&entry.job))
                })
            })
            .flatten();
        self.total_tx_size.sub_or_recompute(
            tx_size,
            recompute,
            "ordered_resolve_queue total_tx_size",
            op,
        );
        Some(entry.job)
    }

    /// Insert a job after duplicate and capacity checks, applying all
    /// accounting (lookup, flight, size, count, notify). Returns the job id
    /// and whether it was newly added. Shared by `add_tx` and
    /// `add_tx_delayed`, which place the id in the FIFO or the delayed heap
    /// respectively.
    fn insert_job(&mut self, job: ResolveJob) -> Result<(ProposalShortId, bool), Reject> {
        let id = job.tx.proposal_short_id();
        if self.lookup.contains_key(&id) {
            return Ok((id, false));
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
        self.lookup.insert(
            id.clone(),
            LookupEntry {
                job,
                generation: self.next_generation,
            },
        );
        self.next_generation += 1;
        // Drop any stale tombstone left for this id by an earlier removal.
        // Tombstones are keyed by id, not by slot: a leftover bit would
        // otherwise drain this freshly added live slot in `pop_front` and
        // strand the job in `lookup` forever (e.g. a delayed job is removed,
        // then the same tx is re-added to the FIFO before the stale heap
        // entry has been cleaned up).
        self.removed.remove(&id);
        self.live_count += 1;
        self.ready_rx.notify_one();
        Ok((id, true))
    }

    /// Returns the first live entry in the queue and removes it.
    ///
    /// FIFO entries are popped first; a delayed job is only returned once
    /// the FIFO has drained and its deadline has passed, which mirrors the
    /// previous behaviour where a retried job was re-enqueued at the back.
    ///
    /// The popped id is moved into `active` and stays visible to
    /// `contains_or_active` until the resolver calls [`Self::finish`].
    pub fn pop_front(&mut self) -> Option<ResolveJob> {
        self.drain_tombstones();
        while let Some(id) = self.inner.pop_front() {
            if let Some(job) = self.remove_live(&id, "pop_front") {
                self.active.insert(id, job.tx.clone());
                return Some(job);
            }
            // Ghost slot: one id can occupy several FIFO slots while `removed`
            // holds only a single tombstone bit for it (remove → re-add →
            // remove). Once the bit has drained the first slot, the remaining
            // slots have no live job in `lookup`; discard them just like stale
            // delayed-heap entries instead of panicking.
            debug!(
                "ordered_resolve_queue: discarding ghost FIFO slot for {}",
                id
            );
        }
        // The FIFO is empty: promote due delayed jobs, discarding stale
        // heap entries (their job was removed and possibly re-added since
        // they were pushed).
        loop {
            let due = matches!(self.delayed.peek(), Some(std::cmp::Reverse((deadline, _, _))) if *deadline <= Instant::now());
            if !due {
                return None;
            }
            let std::cmp::Reverse((_, generation, id)) = self.delayed.pop().expect("peeked");
            let current_generation = self.lookup.get(&id).map(|entry| entry.generation);
            if current_generation != Some(generation) {
                // Stale entry. The id is fully gone: drop its tombstone as
                // well — a delayed job has no FIFO slot, so it would
                // otherwise linger in `removed` forever. The id was
                // re-added (different generation): the current job owns its
                // own state, nothing to clean up here.
                if current_generation.is_none() {
                    self.removed.remove(&id);
                }
                continue;
            }
            let job = self
                .remove_live(&id, "pop_front delayed")
                .expect("generation matched");
            self.active.insert(id, job.tx.clone());
            return Some(job);
        }
    }

    /// Add a job that must not be retried before `delay` has passed.
    ///
    /// The job is fully accounted like a FIFO entry (lookup, flight, size,
    /// count) and is visible to `contains_key` / `get_tx` / `remove_tx`, but
    /// it only becomes poppable at its deadline.
    pub fn add_tx_delayed(&mut self, job: ResolveJob, delay: Duration) -> Result<bool, Reject> {
        let (id, added) = self.insert_job(job)?;
        if added {
            let generation = self.lookup[&id].generation;
            self.delayed
                .push(std::cmp::Reverse((Instant::now() + delay, generation, id)));
        }
        Ok(added)
    }

    /// The deadline of the earliest delayed job, if any.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.delayed
            .peek()
            .map(|std::cmp::Reverse((deadline, _, _))| *deadline)
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

    fn max_queue_tx_size(&self) -> usize {
        crate::constants::MAX_ORDERED_RESOLVE_QUEUE_TX_SIZE
    }

    fn is_empty(&self) -> bool {
        self.live_count == 0
    }

    fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.lookup.contains_key(id)
    }

    fn remove_tx(&mut self, id: &ProposalShortId) -> Option<ResolveJob> {
        let job = self.remove_live(id, "remove_tx")?;
        self.removed.insert(id.clone());
        // Attempt to reclaim the front slot if this id happens to be there.
        self.drain_tombstones();
        self.shrink_to_fit();
        Some(job)
    }

    fn remove_txs_by_peer(&mut self, peer: &PeerIndex) -> Vec<ProposalShortId> {
        // First pass: identify which ids to remove. Iterate `lookup` (not
        // `inner`) so that delayed jobs are covered as well as FIFO jobs.
        let to_remove: Vec<ProposalShortId> = self
            .lookup
            .iter()
            .filter(|(_, entry)| entry.job.source.peer().is_some_and(|p| p == *peer))
            .map(|(id, _)| id.clone())
            .collect();

        for id in &to_remove {
            if self.remove_live(id, "remove_txs_by_peer").is_some() {
                self.removed.insert(id.clone());
            }
        }

        self.drain_tombstones();
        self.shrink_to_fit();
        to_remove
    }

    fn add_tx(&mut self, job: ResolveJob) -> Result<bool, Reject> {
        let (id, added) = self.insert_job(job)?;
        if added {
            self.inner.push_back(id);
        }
        Ok(added)
    }

    fn clear(&mut self) {
        self.inner.clear();
        self.lookup.clear();
        self.removed.clear();
        self.delayed.clear();
        self.live_count = 0;
        self.active.clear();
        self.flight.clear();
        self.total_tx_size.set(0);
        self.shrink_to_fit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::tests::util::build_tx;
    use crate::tx_source::TxSource;
    use ckb_types::packed::Byte32;

    fn job(input_byte: u8) -> ResolveJob {
        let tx = build_tx(vec![(&Byte32::new([input_byte; 32]), 0)], 1);
        ResolveJob::new(tx, TxSource::Local)
    }

    fn job_with_peer(input_byte: u8, peer: u64) -> ResolveJob {
        let tx = build_tx(vec![(&Byte32::new([input_byte; 32]), 0)], 1);
        ResolveJob::new(
            tx,
            TxSource::Remote {
                cycles: 0,
                peer: (peer as usize).into(),
            },
        )
    }

    fn id_of(job: &ResolveJob) -> ProposalShortId {
        job.tx.proposal_short_id()
    }

    /// FIFO entries pop first; a delayed job is only returned once the FIFO
    /// has drained, even when it is already due (the old "retry goes to the
    /// back of the queue" semantics).
    #[test]
    fn fifo_entries_pop_before_due_delayed_jobs() {
        let mut queue = OrderedResolveQueue::new();
        let fifo_job = job(1);
        let delayed_job = job(2);
        let delayed_id = id_of(&delayed_job);

        queue.add_tx(fifo_job.clone()).unwrap();
        queue.add_tx_delayed(delayed_job, Duration::ZERO).unwrap();
        assert_eq!(queue.len(), 2);

        let first = queue.pop_front().expect("fifo job pops first");
        assert_eq!(first.tx.hash(), fifo_job.tx.hash());

        let second = queue.pop_front().expect("due delayed job pops next");
        assert_eq!(second.tx.proposal_short_id(), delayed_id);

        assert!(queue.pop_front().is_none());
        assert_eq!(queue.len(), 0);
        assert!(queue.is_empty());
    }

    /// A delayed job is visible to lookup-style operations but is not
    /// poppable before its deadline.
    #[test]
    fn delayed_job_visible_but_not_poppable_before_deadline() {
        let mut queue = OrderedResolveQueue::new();
        let delayed_job = job(3);
        let delayed_id = id_of(&delayed_job);

        queue
            .add_tx_delayed(delayed_job, Duration::from_secs(3600))
            .unwrap();

        assert!(queue.contains_key(&delayed_id));
        assert!(queue.get_tx(&delayed_id).is_some());
        assert!(!queue.is_empty());
        assert!(queue.next_deadline().is_some());
        assert!(queue.pop_front().is_none());
    }

    /// `next_deadline` reports the earliest delayed deadline.
    #[test]
    fn next_deadline_tracks_earliest_delayed_job() {
        let mut queue = OrderedResolveQueue::new();
        assert!(queue.next_deadline().is_none());

        queue
            .add_tx_delayed(job(4), Duration::from_secs(3600))
            .unwrap();
        queue.add_tx_delayed(job(5), Duration::ZERO).unwrap();

        let deadline = queue.next_deadline().expect("has delayed jobs");
        assert!(deadline <= Instant::now());
    }

    /// A delayed job removed via `remove_tx` becomes a stale heap entry: it
    /// is skipped on pop (never panics, never double-counts) and its
    /// tombstone in `removed` is cleaned up.
    #[test]
    fn remove_tx_of_delayed_job_leaves_no_stale_state() {
        let mut queue = OrderedResolveQueue::new();
        let delayed_job = job(6);
        let delayed_id = id_of(&delayed_job);

        queue.add_tx_delayed(delayed_job, Duration::ZERO).unwrap();
        assert_eq!(queue.len(), 1);

        assert!(queue.remove_tx(&delayed_id).is_some());
        assert_eq!(queue.len(), 0);
        assert!(queue.removed.contains(&delayed_id));

        // The stale heap entry must be skipped without touching accounting,
        // and its tombstone must be dropped rather than linger forever.
        assert!(queue.pop_front().is_none());
        assert!(!queue.removed.contains(&delayed_id));
        assert!(queue.next_deadline().is_none());
    }

    /// `remove_txs_by_peer` covers delayed jobs as well as FIFO jobs.
    #[test]
    fn remove_txs_by_peer_covers_delayed_jobs() {
        let mut queue = OrderedResolveQueue::new();
        let fifo_job = job_with_peer(7, 1);
        let fifo_id = id_of(&fifo_job);
        let delayed_job = job_with_peer(8, 1);
        let delayed_id = id_of(&delayed_job);
        let other_job = job_with_peer(9, 2);
        let other_id = id_of(&other_job);

        queue.add_tx(fifo_job).unwrap();
        queue
            .add_tx_delayed(delayed_job, Duration::from_secs(3600))
            .unwrap();
        queue.add_tx(other_job).unwrap();
        assert_eq!(queue.len(), 3);

        let removed = queue.remove_txs_by_peer(&1.into());
        assert_eq!(removed.len(), 2);
        assert!(removed.contains(&fifo_id));
        assert!(removed.contains(&delayed_id));

        assert_eq!(queue.len(), 1);
        assert!(queue.contains_key(&other_id));
        assert!(!queue.contains_key(&fifo_id));
        assert!(!queue.contains_key(&delayed_id));
    }

    /// The same id can occupy several FIFO slots while `removed` holds only
    /// one tombstone bit for it (remove → re-add → remove). Popping must
    /// discard the surplus ghost slots instead of panicking on the missing
    /// lookup entry.
    #[test]
    fn duplicate_tombstone_slots_never_panic() {
        let mut queue = OrderedResolveQueue::new();
        let job_a = job(10);
        let job_b = job(11);
        let id_b = id_of(&job_b);

        queue.add_tx(job_a.clone()).unwrap();
        queue.add_tx(job_b.clone()).unwrap();
        // Tombstone B while it is *not* at the front, so the bit survives.
        assert!(queue.remove_tx(&id_b).is_some());
        queue.add_tx(job_b).unwrap();
        assert!(queue.remove_tx(&id_b).is_some());
        assert_eq!(queue.len(), 1);

        let first = queue.pop_front().expect("A pops first");
        assert_eq!(first.tx.hash(), job_a.tx.hash());

        // The single tombstone bit drains B's first slot; the second slot is
        // a ghost and must be skipped without touching accounting.
        assert!(queue.pop_front().is_none());
        assert_eq!(queue.len(), 0);
        assert!(queue.is_empty());
        assert!(!queue.removed.contains(&id_b));
    }

    /// A delayed job removed via `remove_tx` leaves a lingering tombstone
    /// (it has no FIFO slot to drain). Re-adding the same id to the FIFO
    /// must clear that bit, otherwise the tombstone would drain the new live
    /// slot and strand the job in `lookup` forever.
    #[test]
    fn readded_after_delayed_removal_pops_normally() {
        let mut queue = OrderedResolveQueue::new();
        let job_x = job(12);
        let id_x = id_of(&job_x);

        queue
            .add_tx_delayed(job_x.clone(), Duration::from_secs(3600))
            .unwrap();
        assert!(queue.remove_tx(&id_x).is_some());
        assert!(queue.removed.contains(&id_x));

        queue.add_tx(job_x.clone()).unwrap();
        assert!(!queue.removed.contains(&id_x));

        let popped = queue.pop_front().expect("re-added job pops");
        assert_eq!(popped.tx.hash(), job_x.tx.hash());
        assert!(queue.pop_front().is_none());
        assert_eq!(queue.len(), 0);
        assert!(queue.is_empty());
    }

    /// A popped job stays visible to `contains_or_active` (but not to
    /// `contains_key`) until the resolver marks it finished.
    #[test]
    fn popped_job_stays_visible_until_finish() {
        let mut queue = OrderedResolveQueue::new();
        let job_a = job(13);
        let id_a = id_of(&job_a);
        queue.add_tx(job_a).unwrap();

        let popped = queue.pop_front().expect("job pops");
        assert_eq!(popped.tx.proposal_short_id(), id_a);
        assert!(!queue.contains_key(&id_a));
        assert!(queue.contains_or_active(&id_a));
        assert_eq!(
            queue.get_active_tx(&id_a).map(|tx| tx.hash()),
            Some(popped.tx.hash())
        );

        queue.finish(&id_a);
        assert!(!queue.contains_or_active(&id_a));
        assert!(queue.get_active_tx(&id_a).is_none());
    }

    /// A delayed job removed and re-added with a later deadline must not be
    /// popped early by the stale heap entry from its first lifetime:
    /// generations tell the two lifetimes apart.
    #[test]
    fn readded_delayed_job_is_not_popped_by_stale_deadline() {
        let mut queue = OrderedResolveQueue::new();
        let job_x = job(14);
        let id_x = id_of(&job_x);

        // First lifetime: due immediately, removed before being popped.
        queue.add_tx_delayed(job_x.clone(), Duration::ZERO).unwrap();
        assert!(queue.remove_tx(&id_x).is_some());

        // Second lifetime: re-added with a far-future deadline.
        queue
            .add_tx_delayed(job_x, Duration::from_secs(3600))
            .unwrap();

        // The stale zero-delay heap entry must not pop the current job early.
        assert!(queue.pop_front().is_none());
        assert!(queue.contains_key(&id_x));
        assert_eq!(queue.len(), 1);
    }
}
