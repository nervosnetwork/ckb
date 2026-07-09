//! Pre-check queue for the tx-pool pipeline.
//!
//! A small multi-consumer queue used by the pre-check worker pool. It is
//! intentionally kept separate from the ordered resolve queue: jobs here are
//! independent and can be processed in any order, while the ordered queue must
//! retry missing-input txs in arrival order.

use crate::component::flight_tracker::FlightTracker;
use crate::component::saturating_counter::SaturatingCounter;
use crate::constants::DEFAULT_MAX_PIPELINE_QUEUE_TX_SIZE;
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
    flight: FlightTracker,
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
                flight: FlightTracker::new(),
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
        state.total_tx_size.get().saturating_add(add_tx_size) >= DEFAULT_MAX_PIPELINE_QUEUE_TX_SIZE
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

    /// Returns the raw transaction for the given id, if it is waiting in the
    /// pre-check queue.
    pub fn get_tx(&self, id: &ProposalShortId) -> Option<TransactionView> {
        let state = self.lock();
        state
            .inner
            .iter()
            .find(|job| &job.tx.proposal_short_id() == id)
            .map(|job| job.tx.clone())
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
        state.flight.remove(id);
        state.total_tx_size.sub_or_zero(
            Self::tx_size(&job),
            "pre_check_queue total_tx_size",
            "remove_by_id",
        );
        Some(job.tx)
    }

    /// Remove all jobs submitted by the given peer.
    pub fn remove_by_peer(&self, peer: &PeerIndex) -> Vec<TransactionView> {
        let mut state = self.lock();
        let to_remove: Vec<usize> = state
            .inner
            .iter()
            .enumerate()
            .filter(|(_, job)| job.source.peer().is_some_and(|p| p == *peer))
            .map(|(idx, _)| idx)
            .collect();

        let mut removed = Vec::with_capacity(to_remove.len());
        for idx in to_remove.into_iter().rev() {
            let job = state.inner.remove(idx).expect("position exists");
            let id = job.tx.proposal_short_id();
            state.index.remove(&id);
            state.flight.remove(&id);
            state.total_tx_size.sub_or_zero(
                Self::tx_size(&job),
                "pre_check_queue total_tx_size",
                "remove_by_peer",
            );
            removed.push(job.tx);
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
        state.flight.insert(id, &job.tx);
        state.inner.push_back(job);
        let new_total = state.total_tx_size.get().saturating_add(tx_size);
        state.total_tx_size.set(new_total);
        drop(state);
        self.ready.notify_one();
        Ok(())
    }

    /// Drain all pending jobs without cancelling the queue.
    pub(crate) fn clear(&self) {
        let mut state = self.lock();
        state.inner.clear();
        state.index.clear();
        state.flight.clear();
        state.total_tx_size.set(0);
    }

    /// Pop the next job, or return `None` if the queue has been cancelled.
    pub(crate) async fn pop(&self) -> Option<PreCheckJob> {
        loop {
            {
                let mut state = self.lock();
                if let Some(job) = state.inner.pop_front() {
                    let id = job.tx.proposal_short_id();
                    state.index.remove(&id);
                    state.flight.remove(&id);
                    let tx_size = Self::tx_size(&job);
                    state.total_tx_size.sub_or_zero(
                        tx_size,
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
}
