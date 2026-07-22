//! Tracks transactions that have been popped by a pipeline worker but have
//! not reached a terminal state yet.

use ckb_types::packed::ProposalShortId;
use std::collections::HashMap;

/// Transactions popped by a worker but not yet terminally processed, keyed
/// by short id.
///
/// A popped job stays "active" until the worker calls
/// [`ActiveSet::finish_if`].
/// If the worker panics in between, the id stays in the set until the
/// owning queue is cleared. That only makes dependent orphans wait out
/// their bounded retry budget; it must never block a genuine resubmission —
/// duplicate detection therefore keeps using the queue's own
/// `contains_key`, and only in-flight visibility checks
/// (`all_missing_parents_in_flight`, read-only pipeline queries) consult
/// this set.
#[derive(Debug, Clone)]
pub(crate) struct ActiveSet<T> {
    inner: HashMap<ProposalShortId, ActiveEntry<T>>,
    next_token: u64,
    exhausted: bool,
}

#[derive(Debug, Clone)]
struct ActiveEntry<T> {
    value: T,
    token: u64,
}

impl<T> Default for ActiveSet<T> {
    fn default() -> Self {
        Self {
            inner: HashMap::new(),
            next_token: 0,
            exhausted: false,
        }
    }
}

impl<T> ActiveSet<T> {
    /// Mark a popped job as being processed.
    pub(crate) fn reserve_token(&mut self) -> Option<u64> {
        if self.exhausted {
            return None;
        }
        let token = self.next_token;
        if let Some(next) = token.checked_add(1) {
            self.next_token = next;
        } else {
            self.exhausted = true;
        }
        Some(token)
    }

    pub(crate) fn insert_reserved(&mut self, id: ProposalShortId, job: T, token: u64) {
        self.inner.insert(id, ActiveEntry { value: job, token });
    }

    /// Finish an active lease only when the stored job still belongs to the
    /// caller. Administrative clear may erase the active map and admit the
    /// same id in a newer pipeline generation before an old worker returns;
    /// an unconditional id-only finish would then erase the newer lease.
    pub(crate) fn finish_if(
        &mut self,
        id: &ProposalShortId,
        owns: impl FnOnce(&T) -> bool,
    ) -> bool {
        if self.inner.get(id).is_some_and(|entry| owns(&entry.value)) {
            self.inner.remove(id);
            true
        } else {
            false
        }
    }

    /// Finish exactly the worker lease returned by the corresponding pop.
    /// Unlike an id/epoch comparison, this remains ABA-safe when one tx is
    /// held and restored multiple times within the same administrative epoch.
    pub(crate) fn finish_token(&mut self, id: &ProposalShortId, token: u64) -> bool {
        if self.inner.get(id).is_some_and(|entry| entry.token == token) {
            self.inner.remove(id);
            true
        } else {
            false
        }
    }

    pub(crate) fn remove_token(&mut self, id: &ProposalShortId, token: u64) -> Option<T> {
        if self.inner.get(id).is_some_and(|entry| entry.token == token) {
            self.inner.remove(id).map(|entry| entry.value)
        } else {
            None
        }
    }

    pub(crate) fn remove_if(
        &mut self,
        id: &ProposalShortId,
        owns: impl FnOnce(&T) -> bool,
    ) -> Option<T> {
        if self.inner.get(id).is_some_and(|entry| owns(&entry.value)) {
            self.inner.remove(id).map(|entry| entry.value)
        } else {
            None
        }
    }

    /// Returns true if the id is currently being processed by a worker.
    pub(crate) fn contains(&self, id: &ProposalShortId) -> bool {
        self.inner.contains_key(id)
    }

    /// Returns the in-flight job for the id, if any.
    pub(crate) fn get(&self, id: &ProposalShortId) -> Option<&T> {
        self.inner.get(id).map(|entry| &entry.value)
    }

    /// Drop all tracked jobs (pipeline clear).
    pub(crate) fn clear(&mut self) {
        self.inner.clear();
    }
}
