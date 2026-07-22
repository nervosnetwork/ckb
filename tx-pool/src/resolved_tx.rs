//! Types for the tx-pool resolve stage.
//!
//! The resolve stage turns a raw [`TransactionView`] into a [`ResolvedTx`] by
//! resolving inputs/cell_deps against the current chain snapshot and the
//! in-pool cell overlay.  It runs as a single ordered worker so that dependent
//! transactions are resolved in the order they arrive.

use crate::component::pool_map::Status;
use crate::tx_source::TxSource;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{Capacity, TransactionView, cell::ResolvedTransaction},
    packed::Byte32,
};
use ckb_verification::cache::Completed;
use std::sync::{Arc, Mutex};

/// Shared budget for fully-resolved transactions that have entered the
/// asynchronous verify/RBF lifecycle.
///
/// Queue-local byte counters are insufficient here: a transaction popped by
/// a verify worker is still resident in the active set, and an in-flight RBF
/// loser moves from the verify queue into `RaceLost` without becoming
/// terminal.  A permit is therefore attached to `ResolvedTx` and follows all
/// of its clones until the last copy is dropped.
#[derive(Debug)]
pub(crate) struct ResolvedTxBudget {
    max_tx_size: usize,
    max_entries: usize,
    state: Mutex<ResolvedTxBudgetState>,
}

#[derive(Debug, Default)]
struct ResolvedTxBudgetState {
    tx_size: usize,
    entries: usize,
}

impl ResolvedTxBudget {
    pub(crate) fn new(max_tx_size: usize, max_entries: usize) -> Arc<Self> {
        Arc::new(Self {
            max_tx_size,
            max_entries,
            state: Mutex::new(ResolvedTxBudgetState::default()),
        })
    }

    fn try_acquire(self: &Arc<Self>, tx_size: usize) -> Option<Arc<ResolvedTxPermit>> {
        let mut state = self.state.lock().unwrap();
        let next_size = state.tx_size.checked_add(tx_size)?;
        let next_entries = state.entries.checked_add(1)?;
        if next_size > self.max_tx_size || next_entries > self.max_entries {
            return None;
        }
        state.tx_size = next_size;
        state.entries = next_entries;
        drop(state);
        Some(Arc::new(ResolvedTxPermit {
            budget: Arc::clone(self),
            tx_size,
        }))
    }

    #[cfg(test)]
    pub(crate) fn usage(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap();
        (state.tx_size, state.entries)
    }
}

/// RAII token for one resident resolved transaction.
#[derive(Debug)]
pub(crate) struct ResolvedTxPermit {
    budget: Arc<ResolvedTxBudget>,
    tx_size: usize,
}

impl Drop for ResolvedTxPermit {
    fn drop(&mut self) {
        let mut state = self.budget.state.lock().unwrap();
        state.tx_size = state.tx_size.saturating_sub(self.tx_size);
        state.entries = state.entries.saturating_sub(1);
    }
}

/// A job submitted to the resolve queue.
#[derive(Debug, Clone)]
pub struct ResolveJob {
    /// The raw transaction to resolve.
    pub tx: TransactionView,
    /// The origin of the transaction (remote, local, or proposal notification).
    pub source: TxSource,
    /// Pipeline generation in which this job was admitted.
    pub epoch: u64,
    /// Number of times this local transaction has been retried because its
    /// inputs were not yet available. Used to bound retries for orphans that
    /// are not satisfiable (`MAX_LOCAL_ORPHAN_ATTEMPTS`) and for orphans whose
    /// parents stay in flight indefinitely
    /// (`MAX_LOCAL_ORPHAN_IN_FLIGHT_ATTEMPTS`).
    pub attempts: u16,
}

impl ResolveJob {
    /// Create a new resolve job for a transaction that has not been retried yet.
    #[cfg(test)]
    pub fn new(tx: TransactionView, source: TxSource) -> Self {
        Self::new_at(tx, source, 0)
    }

    /// Create a new resolve job in an explicit pipeline generation.
    pub(crate) fn new_at(tx: TransactionView, source: TxSource, epoch: u64) -> Self {
        Self {
            tx,
            source,
            epoch,
            attempts: 0,
        }
    }
}

/// A transaction that has been resolved and is ready for verification.
#[derive(Debug, Clone)]
pub struct ResolvedTx {
    /// The raw transaction.
    pub tx: TransactionView,
    /// The resolved transaction.
    pub rtx: Arc<ResolvedTransaction>,
    /// Pool status derived at resolve time.
    pub status: Status,
    /// Transaction fee calculated at resolve time.
    pub fee: Capacity,
    /// Serialized transaction size.
    pub tx_size: usize,
    /// Tip hash at resolve time; used to detect snapshot drift before submit.
    pub pre_resolve_tip: Byte32,
    /// Snapshot used during resolve; reused for verification.
    pub snapshot: Arc<Snapshot>,
    /// The origin of the transaction (remote, local, or proposal notification).
    pub source: TxSource,
    /// Pipeline generation inherited from the resolve job.
    pub(crate) epoch: u64,
    /// Completed script verification carried by authoritative RBF ownership.
    /// A `RaceLost` restore can therefore reuse the result even if the
    /// best-effort global cache-update channel was saturated.
    pub(crate) verified: Option<Completed>,
    /// Lifecycle-wide resource permit. `None` before the transaction first
    /// enters the asynchronous verify queue; queue admission installs it.
    pub(crate) resident_permit: Option<Arc<ResolvedTxPermit>>,
}

impl ResolvedTx {
    /// Ensure this transaction is charged to `budget`. Existing permits are
    /// preserved when a transaction moves queue -> active -> RaceLost -> queue.
    pub(crate) fn ensure_resident(&mut self, budget: &Arc<ResolvedTxBudget>) -> Result<(), ()> {
        if self.resident_permit.is_none() {
            self.resident_permit = budget.try_acquire(self.tx_size);
        }
        self.resident_permit.as_ref().map(|_| ()).ok_or(())
    }
}

// The resource permit is an ownership detail, not transaction identity.
impl PartialEq for ResolvedTx {
    fn eq(&self, other: &Self) -> bool {
        self.tx == other.tx
            && self.rtx == other.rtx
            && self.status == other.status
            && self.fee == other.fee
            && self.tx_size == other.tx_size
            && self.pre_resolve_tip == other.pre_resolve_tip
            && self.snapshot == other.snapshot
            && self.source == other.source
            && self.epoch == other.epoch
            && self.verified == other.verified
    }
}
