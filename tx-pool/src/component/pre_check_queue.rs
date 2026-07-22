//! Pre-check queue for the tx-pool pipeline.
//!
//! A small multi-consumer queue used by the pre-check worker pool. It is
//! intentionally kept separate from the ordered resolve queue: jobs here are
//! independent and can be processed in any order, while the ordered queue must
//! retry missing-input txs in arrival order.

use crate::component::active_set::ActiveSet;
use crate::component::flight_tracker::FlightTracker;
use crate::component::saturating_counter::SaturatingCounter;
use crate::constants::MAX_PRE_CHECK_QUEUE_TX_SIZE;
use crate::error::Reject;
use crate::tx_source::TxSource;
use ckb_network::PeerIndex;
use ckb_stop_handler::CancellationToken;
use ckb_types::core::TransactionView;
use ckb_types::packed::ProposalShortId;
use std::collections::{HashSet, VecDeque};
use std::sync::{Mutex, MutexGuard};

/// A classification job that is offloaded to the pre-check worker pool.
#[derive(Clone)]
pub(crate) struct PreCheckJob {
    pub tx: TransactionView,
    pub source: TxSource,
}

struct PreCheckQueueState {
    inner: VecDeque<PreCheckJob>,
    index: HashSet<ProposalShortId>,
    /// Id-addressable view of the queued jobs, so lookups (`get_tx` in the
    /// compact-block reconstruction hot path) are O(1) instead of a linear
    /// scan of the deque. Kept in sync with `inner` on every mutation.
    lookup: std::collections::HashMap<ProposalShortId, PreCheckJob>,
    flight: FlightTracker,
    /// Jobs that have been popped by a worker but have not reached a
    /// terminal state yet; see [`PreCheckQueue::contains_or_active`].
    active: ActiveSet<TransactionView>,
    /// Total serialized size of all transactions currently in the queue.
    ///
    /// This counter is kept inside the mutex-protected state because every
    /// operation that changes it already holds `state`. There is no need for
    /// atomics: the critical sections are short, never cross `.await`, and
    /// `tokio::sync::Notify` handles asynchronous wake-ups.
    total_tx_size: SaturatingCounter<usize>,
}

pub(crate) struct PreCheckQueue {
    state: Mutex<PreCheckQueueState>,
    ready: tokio::sync::Notify,
    cancel: CancellationToken,
}

impl PreCheckQueue {
    pub(crate) fn new(cancel: CancellationToken) -> Self {
        Self {
            state: Mutex::new(PreCheckQueueState {
                inner: VecDeque::new(),
                index: HashSet::new(),
                lookup: std::collections::HashMap::new(),
                flight: FlightTracker::new(),
                active: ActiveSet::default(),
                total_tx_size: SaturatingCounter::new(0),
            }),
            ready: tokio::sync::Notify::new(),
            cancel,
        }
    }

    fn tx_size(job: &PreCheckJob) -> usize {
        job.tx.data().serialized_size_in_block()
    }

    fn lock(&self) -> MutexGuard<'_, PreCheckQueueState> {
        self.state.lock().expect("pre_check queue lock poisoned")
    }

    /// Returns true if the queue is full.
    ///
    /// Must be called while holding the queue lock so the size check is
    /// consistent with concurrent modifications.
    fn is_full_locked(&self, state: &PreCheckQueueState, add_tx_size: usize) -> bool {
        state.total_tx_size.get().saturating_add(add_tx_size) >= MAX_PRE_CHECK_QUEUE_TX_SIZE
    }

    /// Returns true if the given tx spends or references an output produced by
    /// a transaction currently in the pre-check queue.
    pub fn depends_on(&self, tx: &TransactionView) -> bool {
        let state = self.lock();
        state.flight.depends_on(tx)
    }

    /// Returns true if the queue contains a job for the given proposal id.
    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        let state = self.lock();
        state.index.contains(id)
    }

    /// Returns true if the given proposal id is either queued or currently
    /// being processed by a worker (popped but not yet terminal).
    ///
    /// Introduced for the local-orphan flight check
    /// (`all_missing_parents_in_flight`): a parent transaction must count as
    /// "in flight" for the whole time it is inside the pipeline, including
    /// the window between `pop` and the end of classification, otherwise the
    /// orphan's retry budget is burned while its parent is mid-flight. The
    /// read-only pipeline query paths (`find_tx_in_pipeline`,
    /// `exclude_existing_proposal`, `get_tx_for_compact_block`) use it for
    /// the same visibility.
    ///
    /// Deliberately *not* used for duplicate detection (`contains_key`): if a
    /// worker panics between `pop` and `finish`, the id stays in `active`
    /// until the queue is cleared. That only makes orphans wait out their
    /// bounded retry budget; it must never block a genuine resubmission.
    pub fn contains_or_active(&self, id: &ProposalShortId) -> bool {
        let state = self.lock();
        state.index.contains(id) || state.active.contains(id)
    }

    /// Returns the transaction currently being processed by a worker
    /// (popped but not yet finished), if any.
    pub fn get_active_tx(&self, id: &ProposalShortId) -> Option<TransactionView> {
        let state = self.lock();
        state.active.get(id).cloned()
    }

    /// Mark a previously popped job as terminally processed.
    ///
    /// Every worker must call this exactly once per popped job, after the job
    /// has landed in its terminal state (forwarded, re-enqueued, or
    /// rejected).
    pub fn finish(&self, id: &ProposalShortId) {
        let mut state = self.lock();
        state.active.finish(id);
    }

    /// Returns the raw transaction for the given id, if it is waiting in the
    /// pre-check queue.
    pub fn get_tx(&self, id: &ProposalShortId) -> Option<TransactionView> {
        let state = self.lock();
        state.lookup.get(id).map(|job| job.tx.clone())
    }

    /// Remove a job from the queue by its short id.
    pub fn remove_by_id(&self, id: &ProposalShortId) -> Option<TransactionView> {
        let mut state = self.lock();
        let pos = state
            .inner
            .iter()
            .position(|job| &job.tx.proposal_short_id() == id)?;
        let job = state.inner.remove(pos).expect("position exists");
        state.index.remove(id);
        state.lookup.remove(id);
        state.flight.remove(id);
        let tx_size = Self::tx_size(&job);
        // On underflow (an accounting bug elsewhere), restore the true
        // total from what remains instead of clamping to zero, which
        // would silently disable the size budget.
        let recompute = state
            .total_tx_size
            .get()
            .checked_sub(tx_size)
            .is_none()
            .then(|| {
                state
                    .inner
                    .iter()
                    .try_fold(0usize, |acc, job| acc.checked_add(Self::tx_size(job)))
            })
            .flatten();
        state.total_tx_size.sub_or_recompute(
            tx_size,
            recompute,
            "pre_check_queue total_tx_size",
            "remove_by_id",
        );
        Some(job.tx)
    }

    /// Remove all jobs submitted by the given peer.
    ///
    /// Runs in a single linear pass over the queue (`O(n)`) and preserves the
    /// relative order of the jobs that remain. The total size counter and the
    /// auxiliary indexes are updated while iterating.
    pub fn remove_by_peer(&self, peer: &PeerIndex) -> Vec<TransactionView> {
        let mut state = self.lock();
        let mut removed = Vec::new();
        // Split the borrow so `retain` can mutate `inner` while the closure
        // updates the auxiliary indexes.
        let PreCheckQueueState {
            inner,
            index,
            lookup,
            flight,
            total_tx_size,
            ..
        } = &mut *state;
        // `VecDeque::retain` preserves the relative order of kept jobs and
        // avoids the O(n²) cost of repeated `remove(idx)` on a deque.
        inner.retain(|job| {
            if job.source.peer().is_some_and(|p| p == *peer) {
                let id = job.tx.proposal_short_id();
                index.remove(&id);
                lookup.remove(&id);
                flight.remove(&id);
                removed.push(job.tx.clone());
                false
            } else {
                true
            }
        });
        // Recompute the counter exactly from what remains: exact and O(n)
        // once, versus per-job saturating subtraction inside the loop.
        let remaining = inner
            .iter()
            .try_fold(0usize, |acc, job| acc.checked_add(Self::tx_size(job)));
        if let Some(remaining) = remaining {
            total_tx_size.set(remaining);
        }
        removed
    }

    pub(crate) fn push(&self, job: PreCheckJob) -> Result<(), Reject> {
        let mut state = self.lock();
        let id = job.tx.proposal_short_id();
        if state.index.contains(&id) {
            return Ok(());
        }
        let tx_size = Self::tx_size(&job);
        let tx_hash = job.tx.hash();
        // The full check is performed while holding the lock so concurrent
        // pushes cannot both observe a non-full queue and exceed the limit.
        if self.is_full_locked(&state, tx_size) {
            return Err(Reject::Full(format!(
                "pre_check_queue total_tx_size exceeded, failed to add tx: {tx_hash:#x}"
            )));
        }
        state.index.insert(id.clone());
        state.lookup.insert(id.clone(), job.clone());
        state.flight.insert(id, &job.tx);
        state.inner.push_back(job);
        state
            .total_tx_size
            .add_saturating(tx_size, "pre_check_queue total_tx_size", "push");
        drop(state);
        self.ready.notify_one();
        Ok(())
    }

    /// Drain all pending jobs without cancelling the queue.
    pub(crate) fn clear(&self) {
        let mut state = self.lock();
        state.inner.clear();
        state.index.clear();
        state.lookup.clear();
        state.flight.clear();
        state.active.clear();
        state.total_tx_size.set(0);
    }

    /// Pop the next job, or return `None` if the queue has been cancelled.
    ///
    /// The popped id is moved into `active` and stays visible to
    /// `contains_or_active` until the worker calls [`Self::finish`].
    pub(crate) async fn pop(&self) -> Option<PreCheckJob> {
        loop {
            {
                let mut state = self.lock();
                if let Some(job) = state.inner.pop_front() {
                    let id = job.tx.proposal_short_id();
                    state.index.remove(&id);
                    state.lookup.remove(&id);
                    state.flight.remove(&id);
                    state.active.insert(id, job.tx.clone());
                    let tx_size = Self::tx_size(&job);
                    // On underflow, restore the true total from what
                    // remains queued instead of clamping to zero.
                    let recompute = state
                        .total_tx_size
                        .get()
                        .checked_sub(tx_size)
                        .is_none()
                        .then(|| {
                            state
                                .inner
                                .iter()
                                .try_fold(0usize, |acc, job| acc.checked_add(Self::tx_size(job)))
                        })
                        .flatten();
                    state.total_tx_size.sub_or_recompute(
                        tx_size,
                        recompute,
                        "pre_check_queue total_tx_size",
                        "pop",
                    );
                    return Some(job);
                }
            }
            tokio::select! {
                _ = self.ready.notified() => {}
                _ = self.cancel.cancelled() => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_test_chain_utils::always_success_cell;
    use ckb_types::{
        bytes::Bytes,
        core::{Capacity, TransactionBuilder},
        h256,
        packed::{CellInput, CellOutput, OutPoint},
        prelude::*,
    };

    fn dummy_tx(input: &OutPoint, output_capacity: usize) -> TransactionView {
        let (_, _, always_success_script) = always_success_cell();
        TransactionBuilder::default()
            .input(CellInput::new(input.clone(), 0))
            .output(
                CellOutput::new_builder()
                    .capacity(Capacity::bytes(output_capacity).unwrap())
                    .lock(always_success_script.clone())
                    .build(),
            )
            .output_data(Bytes::default().pack())
            .build()
    }

    #[tokio::test]
    async fn popped_job_stays_visible_until_finish() {
        let cancel = CancellationToken::new();
        let queue = PreCheckQueue::new(cancel);
        let input = OutPoint::new(
            h256!("0x0303030303030303030303030303030303030303030303030303030303030303").pack(),
            0,
        );
        let tx = dummy_tx(&input, 1_000);
        let id = tx.proposal_short_id();
        queue
            .push(PreCheckJob {
                tx: tx.clone(),
                source: TxSource::Local,
            })
            .unwrap();

        let job = queue.pop().await.expect("job pops");
        assert_eq!(job.tx.hash(), tx.hash());
        // No longer queued, but still in flight while the worker classifies it.
        assert!(!queue.contains_key(&id));
        assert!(queue.contains_or_active(&id));

        queue.finish(&id);
        assert!(!queue.contains_or_active(&id));
    }

    #[test]
    fn remove_by_id_and_peer() {
        let cancel = CancellationToken::new();
        let queue = PreCheckQueue::new(cancel);
        let input = OutPoint::new(
            h256!("0x0101010101010101010101010101010101010101010101010101010101010101").pack(),
            0,
        );

        let tx_a = dummy_tx(&input, 1_000);
        let tx_b = dummy_tx(&OutPoint::new(tx_a.hash(), 0), 500);
        let tx_c = dummy_tx(&OutPoint::new(tx_b.hash(), 0), 400);

        queue
            .push(PreCheckJob {
                tx: tx_a.clone(),
                source: TxSource::Remote {
                    cycles: 0,
                    peer: 1.into(),
                },
            })
            .unwrap();
        queue
            .push(PreCheckJob {
                tx: tx_b.clone(),
                source: TxSource::Remote {
                    cycles: 0,
                    peer: 2.into(),
                },
            })
            .unwrap();
        queue
            .push(PreCheckJob {
                tx: tx_c.clone(),
                source: TxSource::Remote {
                    cycles: 0,
                    peer: 1.into(),
                },
            })
            .unwrap();

        assert!(queue.contains_key(&tx_b.proposal_short_id()));
        assert_eq!(
            queue.remove_by_id(&tx_b.proposal_short_id()),
            Some(tx_b.clone())
        );
        assert!(!queue.contains_key(&tx_b.proposal_short_id()));
        assert!(queue.contains_key(&tx_a.proposal_short_id()));
        assert!(queue.contains_key(&tx_c.proposal_short_id()));

        let removed = queue.remove_by_peer(&1.into());
        assert_eq!(removed.len(), 2);
        assert!(removed.iter().any(|tx| tx.hash() == tx_a.hash()));
        assert!(removed.iter().any(|tx| tx.hash() == tx_c.hash()));
        assert!(queue.get_tx(&tx_a.proposal_short_id()).is_none());
        assert!(queue.get_tx(&tx_c.proposal_short_id()).is_none());
    }

    #[test]
    fn remove_by_peer_preserves_order_and_updates_size() {
        let cancel = CancellationToken::new();
        let queue = PreCheckQueue::new(cancel);
        let input = OutPoint::new(
            h256!("0x0202020202020202020202020202020202020202020202020202020202020202").pack(),
            0,
        );

        let tx_a = dummy_tx(&input, 1_000);
        let tx_b = dummy_tx(&OutPoint::new(tx_a.hash(), 0), 500);
        let tx_c = dummy_tx(&OutPoint::new(tx_b.hash(), 0), 400);
        let tx_d = dummy_tx(&OutPoint::new(tx_c.hash(), 0), 300);

        let jobs = [
            (tx_a.clone(), 1),
            (tx_b.clone(), 2),
            (tx_c.clone(), 1),
            (tx_d.clone(), 2),
        ];
        let mut expected_total = 0usize;
        for (tx, peer) in &jobs {
            queue
                .push(PreCheckJob {
                    tx: tx.clone(),
                    source: TxSource::Remote {
                        cycles: 0,
                        peer: (*peer).into(),
                    },
                })
                .unwrap();
            expected_total += PreCheckQueue::tx_size(&PreCheckJob {
                tx: tx.clone(),
                source: TxSource::Local,
            });
        }

        let initial_size = queue.lock().total_tx_size.get();
        assert_eq!(initial_size, expected_total);

        // Remove peer 1: tx_a and tx_c should go, tx_b and tx_d stay in order.
        let removed = queue.remove_by_peer(&1.into());
        assert_eq!(removed.len(), 2);
        assert_eq!(removed[0].hash(), tx_a.hash());
        assert_eq!(removed[1].hash(), tx_c.hash());

        let state = queue.lock();
        assert_eq!(state.inner.len(), 2);
        assert_eq!(state.inner[0].tx.hash(), tx_b.hash());
        assert_eq!(state.inner[1].tx.hash(), tx_d.hash());

        let expected_remaining = PreCheckQueue::tx_size(&PreCheckJob {
            tx: tx_b,
            source: TxSource::Local,
        }) + PreCheckQueue::tx_size(&PreCheckJob {
            tx: tx_d,
            source: TxSource::Local,
        });
        assert_eq!(state.total_tx_size.get(), expected_remaining);
    }
}
