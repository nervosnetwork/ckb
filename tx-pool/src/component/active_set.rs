//! Tracks transactions that have been popped by a pipeline worker but have
//! not reached a terminal state yet.

use ckb_types::packed::ProposalShortId;
use std::collections::HashMap;

/// Transactions popped by a worker but not yet terminally processed, keyed
/// by short id.
///
/// A popped job stays "active" until the worker calls [`ActiveSet::finish`].
/// If the worker panics in between, the id stays in the set until the
/// owning queue is cleared. That only makes dependent orphans wait out
/// their bounded retry budget; it must never block a genuine resubmission —
/// duplicate detection therefore keeps using the queue's own
/// `contains_key`, and only in-flight visibility checks
/// (`all_missing_parents_in_flight`, read-only pipeline queries) consult
/// this set.
#[derive(Debug, Clone)]
pub(crate) struct ActiveSet<T> {
    inner: HashMap<ProposalShortId, T>,
}

impl<T> Default for ActiveSet<T> {
    fn default() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }
}

impl<T> ActiveSet<T> {
    /// Mark a popped job as being processed.
    pub(crate) fn insert(&mut self, id: ProposalShortId, job: T) {
        self.inner.insert(id, job);
    }

    /// Mark a previously popped job as terminally processed.
    ///
    /// Every worker must call this exactly once per popped job, after the
    /// job has landed in its terminal state (forwarded, re-enqueued,
    /// rejected, or panicked).
    pub(crate) fn finish(&mut self, id: &ProposalShortId) {
        self.inner.remove(id);
    }

    /// Returns true if the id is currently being processed by a worker.
    pub(crate) fn contains(&self, id: &ProposalShortId) -> bool {
        self.inner.contains_key(id)
    }

    /// Returns the in-flight job for the id, if any.
    pub(crate) fn get(&self, id: &ProposalShortId) -> Option<&T> {
        self.inner.get(id)
    }

    /// Drop all tracked jobs (pipeline clear).
    pub(crate) fn clear(&mut self) {
        self.inner.clear();
    }
}
