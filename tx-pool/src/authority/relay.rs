//! Bounded handoff from committed tx-pool effects to the relayer projection.
//!
//! Relay state is derived and must never become an authority progress engine.
//! The sole effect publisher therefore performs one nonblocking mailbox Apply;
//! a slow or absent relayer cannot retain an effect lease, compute capability,
//! authority guard, or shutdown edge.

use super::{
    model::{DependencyKey, Entry, Phase},
    store::{MissingCursor, MissingPage, Store},
};
use crate::{service::TxVerificationResult, util::compact_packed};
use ckb_types::packed::Byte32;
use ckb_util::Mutex;
use std::{
    collections::{HashSet, VecDeque},
    mem::size_of,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Notify;

const MIN_RELAY_MAILBOX_ITEMS: usize = 2;
const RELAY_PARENT_SLOT_OVERHEAD: usize = size_of::<u64>() + (2 * size_of::<usize>());

/// Initial publication and reset reconstruction project the same waiting fact.
pub(super) fn unknown_parents(entry: &Entry) -> Option<TxVerificationResult> {
    let Phase::Waiting(keys) = &entry.phase else {
        return None;
    };
    let peer = entry.source.residency_peer()?;
    Some(TxVerificationResult::UnknownParents {
        peer,
        parents: keys
            .iter()
            .filter_map(|key| match key {
                DependencyKey::Cell(point) => Some(compact_packed(&point.tx_hash())),
                DependencyKey::Header(_) => None,
            })
            .collect(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RelayMailboxConfigError {
    ItemLimit,
    ByteLimit,
    Allocation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RelayMailboxDisposition {
    Exact,
    Reconciled,
    Unavailable,
    Disconnected,
}

struct RelayMailboxState {
    queue: VecDeque<TxVerificationResult>,
    bytes: usize,
}

impl RelayMailboxState {
    /// Capacity and the queue ledger change together, or the caller keeps
    /// the original result for reconciliation. The returned flag records a
    /// watermark crossing before insertion.
    fn try_push(
        &mut self,
        result: TxVerificationResult,
        limits: &RelayMailboxInner,
    ) -> Result<bool, TxVerificationResult> {
        let next = relay_result_bytes(&result).and_then(|result_bytes| {
            if self.queue.len() >= limits.max_items {
                return None;
            }
            self.bytes
                .checked_add(result_bytes)
                .filter(|bytes| *bytes <= limits.max_bytes)
        });
        let Some(bytes) = next else {
            return Err(result);
        };
        let crossed_watermark = (self.queue.len() < limits.wake_items
            && self
                .queue
                .len()
                .checked_add(1)
                .is_some_and(|items| items >= limits.wake_items))
            || (self.bytes < limits.wake_bytes && bytes >= limits.wake_bytes);
        self.bytes = bytes;
        self.queue.push_back(result);
        Ok(crossed_watermark)
    }

    fn replace_with_reset(&mut self) {
        if let Some(metrics) = ckb_metrics::handle() {
            metrics
                .ckb_relay_tx_verify_result_queue_resets
                .capacity
                .inc();
        }
        self.queue.clear();
        self.bytes = size_of::<TxVerificationResult>();
        self.queue.push_back(TxVerificationResult::GenerationReset);
    }

    fn pop_front(&mut self) -> Option<TxVerificationResult> {
        let result = self.queue.pop_front()?;
        // Queued messages are immutable; their constant-time charge needs no
        // second stored copy beside the message.
        let Some(bytes) =
            relay_result_bytes(&result).and_then(|bytes| self.bytes.checked_sub(bytes))
        else {
            return Some(self.reset_after_accounting_mismatch());
        };
        if self.queue.is_empty() != (bytes == 0) {
            return Some(self.reset_after_accounting_mismatch());
        }
        self.bytes = bytes;
        Some(result)
    }

    fn reset_after_accounting_mismatch(&mut self) -> TxVerificationResult {
        // This mailbox is a rebuildable projection. If its private byte
        // ledger ever disagrees with its owned messages, discard the
        // remaining detail and force an authoritative relay rebuild. The
        // empty/zero equivalence detects both undercount and overcount without
        // scanning the queue; never hide either mismatch with saturation.
        if let Some(metrics) = ckb_metrics::handle() {
            metrics
                .ckb_relay_tx_verify_result_queue_resets
                .accounting
                .inc();
        }
        self.queue.clear();
        self.bytes = 0;
        TxVerificationResult::GenerationReset
    }
}

struct RelayMailboxInner {
    state: Mutex<RelayMailboxState>,
    receiver_alive: AtomicBool,
    drain_signal: Notify,
    max_items: usize,
    max_bytes: usize,
    wake_items: usize,
    wake_bytes: usize,
}

/// Move-only, nonblocking publication half owned by the sole effect endpoint.
/// Keeping this capability non-cloneable makes multiple relay publishers
/// unrepresentable even though the bounded mailbox storage is shared with its
/// receiver.
pub(crate) struct AuthorityRelaySink {
    inner: Arc<RelayMailboxInner>,
}

/// Sole drain half transferred to the relayer projection during assembly.
pub(super) struct AuthorityRelayReceiver {
    inner: Arc<RelayMailboxInner>,
}

pub(super) fn authority_relay_mailbox(
    max_items: usize,
    max_bytes: usize,
    max_parents: usize,
) -> Result<(AuthorityRelaySink, AuthorityRelayReceiver), RelayMailboxConfigError> {
    if max_items < MIN_RELAY_MAILBOX_ITEMS {
        return Err(RelayMailboxConfigError::ItemLimit);
    }
    let parent_frontier_bytes = relay_parent_frontier_bytes(max_parents)?;
    let minimum_bytes = relay_result_bytes(&TxVerificationResult::GenerationReset)
        .and_then(|reset| reset.checked_add(parent_frontier_bytes))
        .ok_or(RelayMailboxConfigError::ByteLimit)?;
    if max_bytes < minimum_bytes {
        return Err(RelayMailboxConfigError::ByteLimit);
    }
    let mut queue = VecDeque::new();
    queue
        .try_reserve_exact(max_items)
        .map_err(|_| RelayMailboxConfigError::Allocation)?;
    let inner = Arc::new(RelayMailboxInner {
        state: Mutex::new(RelayMailboxState { queue, bytes: 0 }),
        receiver_alive: AtomicBool::new(true),
        drain_signal: Notify::new(),
        max_items,
        max_bytes,
        wake_items: max_items.div_ceil(2),
        wake_bytes: max_bytes.div_ceil(2),
    });
    crate::metrics::relay_queue(0, max_items);
    Ok((
        AuthorityRelaySink {
            inner: Arc::clone(&inner),
        },
        AuthorityRelayReceiver { inner },
    ))
}

/// Construct the production mailbox at its exact indivisible payload bound.
///
/// One maximal missing-parent frontier must fit behind an ordered reset. Small
/// outcomes are bounded independently by `max_items`; provisioning additional
/// bytes would not strengthen liveness and would make the derived relay
/// projection compete with transaction residency.
pub(super) fn production_authority_relay_mailbox(
    max_items: usize,
    max_parents: usize,
) -> Result<(AuthorityRelaySink, AuthorityRelayReceiver), RelayMailboxConfigError> {
    let max_bytes = relay_result_bytes(&TxVerificationResult::GenerationReset)
        .and_then(|reset| {
            relay_parent_frontier_bytes(max_parents)
                .ok()?
                .checked_add(reset)
        })
        .ok_or(RelayMailboxConfigError::ByteLimit)?;
    authority_relay_mailbox(max_items, max_bytes, max_parents)
}

fn relay_parent_frontier_bytes(max_parents: usize) -> Result<usize, RelayMailboxConfigError> {
    let mut parents = HashSet::<Byte32>::new();
    parents
        .try_reserve(max_parents)
        .map_err(|_| RelayMailboxConfigError::Allocation)?;
    parents
        .capacity()
        .checked_mul(
            size_of::<Byte32>()
                .checked_add(RELAY_PARENT_SLOT_OVERHEAD)
                .ok_or(RelayMailboxConfigError::ByteLimit)?,
        )
        .ok_or(RelayMailboxConfigError::ByteLimit)?
        .checked_add(size_of::<TxVerificationResult>())
        .ok_or(RelayMailboxConfigError::ByteLimit)
}

impl AuthorityRelaySink {
    /// Publish one committed relay result without waiting for a consumer.
    ///
    /// Overflow clears only older derived detail, installs one reset before
    /// the current result, and retains the queue allocation. Unknown-parent
    /// detail that cannot fit even after reconciliation is a bounded Remote
    /// availability loss, not a tx-pool authority failure.
    pub(super) fn publish(&self, result: TxVerificationResult) -> RelayMailboxDisposition {
        if !self.inner.receiver_alive.load(Ordering::Acquire) {
            return RelayMailboxDisposition::Disconnected;
        }
        let mut state = self.inner.state.lock();
        if !self.inner.receiver_alive.load(Ordering::Acquire) {
            return RelayMailboxDisposition::Disconnected;
        }
        let prompt = matches!(
            result,
            TxVerificationResult::GenerationReset | TxVerificationResult::UnknownParents { .. }
        );
        let result = match state.try_push(result, &self.inner) {
            Ok(crossed_watermark) => {
                crate::metrics::relay_queue(state.queue.len(), self.inner.max_items);
                drop(state);
                if prompt || crossed_watermark {
                    self.inner.drain_signal.notify_one();
                }
                return RelayMailboxDisposition::Exact;
            }
            Err(result) => result,
        };

        state.replace_with_reset();

        let discarded = if matches!(result, TxVerificationResult::GenerationReset) {
            Some(result)
        } else {
            state.try_push(result, &self.inner).err()
        };
        let disposition = if matches!(
            discarded.as_ref(),
            Some(TxVerificationResult::UnknownParents { .. })
        ) {
            RelayMailboxDisposition::Unavailable
        } else {
            // Reset conservatively clears known/pending relay state for an
            // ordinary Ok/Reject result that cannot itself fit.
            RelayMailboxDisposition::Reconciled
        };
        crate::metrics::relay_queue(state.queue.len(), self.inner.max_items);
        drop(state);
        self.inner.drain_signal.notify_one();
        // Rejected detail can own a large parent frontier. Preserve its release
        // after unlocking and notifying, just as for the original input value.
        drop(discarded);
        disposition
    }
}

impl AuthorityRelayReceiver {
    pub(super) async fn wait_for_drain(&self) {
        self.inner.drain_signal.notified().await;
    }

    pub(super) fn try_recv(&self) -> Option<TxVerificationResult> {
        let mut state = self.inner.state.lock();
        let result = state.pop_front();
        crate::metrics::relay_queue(state.queue.len(), self.inner.max_items);
        result
    }

    /// Reserve outside the mailbox lock, then move one bounded prefix while
    /// holding it once. A concurrent reset may shorten or replace that prefix.
    pub(super) fn drain(&self, limit: usize) -> Vec<TxVerificationResult> {
        let count = self.inner.state.lock().queue.len().min(limit);
        let mut drained = Vec::new();
        if count == 0 || drained.try_reserve(count).is_err() {
            return drained;
        }
        let mut state = self.inner.state.lock();
        for _ in 0..count {
            let Some(result) = state.pop_front() else {
                break;
            };
            drained.push(result);
        }
        crate::metrics::relay_queue(state.queue.len(), self.inner.max_items);
        drained
    }
}

#[cfg(test)]
impl AuthorityRelayReceiver {
    pub(in crate::authority) fn observation(&self) -> (usize, usize) {
        let state = self.inner.state.lock();
        (state.queue.len(), state.bytes)
    }

    pub(in crate::authority) fn corrupt_bytes_for_test(&self, bytes: usize) {
        self.inner.state.lock().bytes = bytes;
    }
}

impl Drop for AuthorityRelayReceiver {
    fn drop(&mut self) {
        self.inner.receiver_alive.store(false, Ordering::Release);
        let mut state = self.inner.state.lock();
        state.queue.clear();
        state.bytes = 0;
        crate::metrics::relay_queue(0, self.inner.max_items);
    }
}

/// A reset may discard an UnknownParents event. Rebuild the still-live waiting
/// levels in bounded pages only when the mailbox has drained; no relay task or
/// extra transaction ownership is required.
pub(crate) struct RelayDrain {
    receiver: AuthorityRelayReceiver,
    store: Weak<Store>,
    cursor: Mutex<Option<MissingCursor>>,
}
const REBUILD_PAGES_PER_RECEIVE: usize = 4;
const REBUILD_PAGE_SIZE: usize = 64;

impl RelayDrain {
    pub(super) fn new(receiver: AuthorityRelayReceiver, store: &Arc<Store>) -> Self {
        Self {
            receiver,
            store: Arc::downgrade(store),
            cursor: Mutex::new(None),
        }
    }

    pub(crate) fn try_recv(&self) -> Option<TxVerificationResult> {
        // Bound reconstruction work even when a page contains no remote waiter.
        (0..REBUILD_PAGES_PER_RECEIVE).find_map(|_| self.try_recv_page())
    }

    fn try_recv_page(&self) -> Option<TxVerificationResult> {
        if let Some(result) = self.receiver.try_recv() {
            if matches!(result, TxVerificationResult::GenerationReset) {
                self.reset_cursor();
            }
            return Some(result);
        }
        let Some(store) = self.store.upgrade() else {
            *self.cursor.lock() = None;
            return None;
        };
        let mut cursor = self.cursor.lock();
        let current = cursor.as_mut()?;
        match store.next_missing(current, REBUILD_PAGE_SIZE) {
            MissingPage::Waiter(result) => Some(result),
            MissingPage::Incomplete => None,
            MissingPage::Exhausted => {
                *cursor = None;
                None
            }
        }
    }

    fn reset_cursor(&self) {
        *self.cursor.lock() = self
            .store
            .upgrade()
            .map(|store| MissingCursor::new(store.snapshot().0));
    }

    pub(crate) fn drain(&self, limit: usize) -> Vec<TxVerificationResult> {
        let mut drained = self.receiver.drain(limit);
        // No reconstruction or external consumer runs within the raw prefix,
        // so only its final reset determines the following rebuild cursor.
        if drained
            .iter()
            .any(|result| matches!(result, TxVerificationResult::GenerationReset))
        {
            self.reset_cursor();
        }
        // One batch gets one receive's page budget, regardless of `limit`.
        for _ in 0..REBUILD_PAGES_PER_RECEIVE {
            if drained.len() == limit {
                break;
            }
            // Reserve before consuming newly arrived data or a rebuild result.
            // Failure leaves every unobserved result with its current owner.
            if drained.try_reserve(1).is_err() {
                break;
            }
            if let Some(result) = self.try_recv_page() {
                drained.push(result);
            }
        }
        drained
    }

    pub(crate) async fn wait_for_drain(&self) {
        if self.cursor.lock().is_some() {
            return;
        }
        self.receiver.wait_for_drain().await;
    }
}

fn relay_result_bytes(result: &TxVerificationResult) -> Option<usize> {
    let envelope = size_of::<TxVerificationResult>();
    match result {
        TxVerificationResult::UnknownParents { parents, .. } => parents
            .capacity()
            .checked_mul(size_of::<Byte32>().checked_add(RELAY_PARENT_SLOT_OVERHEAD)?)?
            .checked_add(envelope),
        TxVerificationResult::Ok { .. }
        | TxVerificationResult::Reject { .. }
        | TxVerificationResult::GenerationReset => Some(envelope),
    }
}
