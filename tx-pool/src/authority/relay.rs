//! Bounded handoff from committed tx-pool effects to the relayer projection.
//!
//! Relay state is derived and must never become an authority progress engine.
//! The sole effect publisher therefore performs one nonblocking mailbox Apply;
//! a slow or absent relayer cannot retain an effect lease, compute capability,
//! authority guard, or shutdown edge.

use super::effect::ParentTransactionRequest;
use crate::{service::TxVerificationResult, util::compact_packed};
use ckb_types::packed::Byte32;
use ckb_util::Mutex;
use std::{
    collections::{HashSet, VecDeque},
    mem::size_of,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Notify;

const MIN_RELAY_MAILBOX_ITEMS: usize = 2;
const RELAY_PARENT_SLOT_OVERHEAD: usize = size_of::<u64>() + (2 * size_of::<usize>());

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

struct RelayEnvelope {
    result: TxVerificationResult,
    bytes: usize,
}

struct RelayMailboxState {
    queue: VecDeque<RelayEnvelope>,
    bytes: usize,
}

impl RelayMailboxState {
    fn pop_front(&mut self) -> Option<TxVerificationResult> {
        let envelope = self.queue.pop_front()?;
        let Some(bytes) = self.bytes.checked_sub(envelope.bytes) else {
            return Some(self.reset_after_accounting_mismatch());
        };
        if self.queue.is_empty() != (bytes == 0) {
            return Some(self.reset_after_accounting_mismatch());
        }
        self.bytes = bytes;
        Some(envelope.result)
    }

    fn reset_after_accounting_mismatch(&mut self) -> TxVerificationResult {
        // This mailbox is a rebuildable projection. If its private byte
        // ledger ever disagrees with its owned envelopes, discard the
        // remaining detail and force an authoritative relay rebuild. The
        // empty/zero equivalence detects both undercount and overcount without
        // scanning the queue; never hide either mismatch with saturation.
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
        let Some(result_bytes) = relay_result_bytes(&result) else {
            return self.reconcile_without_current(result);
        };
        let mut state = self.inner.state.lock();
        if !self.inner.receiver_alive.load(Ordering::Acquire) {
            return RelayMailboxDisposition::Disconnected;
        }
        if let Some(bytes) = mailbox_bytes_after(&state, result_bytes, &self.inner) {
            let prompt = matches!(
                result,
                TxVerificationResult::GenerationReset | TxVerificationResult::UnknownParents { .. }
            );
            let crossed_watermark = relay_drain_watermark_crossed(&state, bytes, &self.inner);
            state.bytes = bytes;
            state.queue.push_back(RelayEnvelope {
                result,
                bytes: result_bytes,
            });
            drop(state);
            if prompt || crossed_watermark {
                self.inner.drain_signal.notify_one();
            }
            return RelayMailboxDisposition::Exact;
        }

        state.queue.clear();
        state.bytes = 0;
        let reset = TxVerificationResult::GenerationReset;
        let Some(reset_bytes) = relay_result_bytes(&reset) else {
            return RelayMailboxDisposition::Unavailable;
        };
        state.bytes = reset_bytes;
        state.queue.push_back(RelayEnvelope {
            result: reset,
            bytes: reset_bytes,
        });

        let disposition = if matches!(result, TxVerificationResult::GenerationReset) {
            RelayMailboxDisposition::Reconciled
        } else if let Some(bytes) = mailbox_bytes_after(&state, result_bytes, &self.inner) {
            state.bytes = bytes;
            state.queue.push_back(RelayEnvelope {
                result,
                bytes: result_bytes,
            });
            RelayMailboxDisposition::Reconciled
        } else if matches!(result, TxVerificationResult::UnknownParents { .. }) {
            RelayMailboxDisposition::Unavailable
        } else {
            // Reset conservatively clears known/pending relay state for an
            // ordinary Ok/Reject result that cannot itself fit.
            RelayMailboxDisposition::Reconciled
        };
        drop(state);
        self.inner.drain_signal.notify_one();
        disposition
    }

    fn reconcile_without_current(&self, result: TxVerificationResult) -> RelayMailboxDisposition {
        let mut state = self.inner.state.lock();
        if !self.inner.receiver_alive.load(Ordering::Acquire) {
            return RelayMailboxDisposition::Disconnected;
        }
        state.queue.clear();
        state.bytes = 0;
        let reset = TxVerificationResult::GenerationReset;
        let Some(reset_bytes) = relay_result_bytes(&reset) else {
            return RelayMailboxDisposition::Unavailable;
        };
        state.bytes = reset_bytes;
        state.queue.push_back(RelayEnvelope {
            result: reset,
            bytes: reset_bytes,
        });
        let disposition = if matches!(result, TxVerificationResult::UnknownParents { .. }) {
            RelayMailboxDisposition::Unavailable
        } else {
            RelayMailboxDisposition::Reconciled
        };
        drop(state);
        self.inner.drain_signal.notify_one();
        disposition
    }
}

impl AuthorityRelayReceiver {
    pub(super) async fn wait_for_drain(&self) {
        self.inner.drain_signal.notified().await;
    }

    pub(super) fn try_recv(&self) -> Option<TxVerificationResult> {
        self.inner.state.lock().pop_front()
    }
}

#[cfg(test)]
#[path = "tests/support/relay.rs"]
mod test_support;

/// Compile one bounded authority-owned parent request into the sync projection.
pub(super) fn project_parent_request(request: &ParentTransactionRequest) -> TxVerificationResult {
    let mut parents = HashSet::with_capacity(request.parents().len());
    parents.extend(
        request
            .parents()
            .iter()
            .map(|parent| compact_packed(&parent.0)),
    );
    TxVerificationResult::UnknownParents {
        peer: request.peer(),
        parents,
    }
}

impl Drop for AuthorityRelayReceiver {
    fn drop(&mut self) {
        self.inner.receiver_alive.store(false, Ordering::Release);
        let mut state = self.inner.state.lock();
        state.queue.clear();
        state.bytes = 0;
    }
}

fn mailbox_bytes_after(
    state: &RelayMailboxState,
    result_bytes: usize,
    limits: &RelayMailboxInner,
) -> Option<usize> {
    if state.queue.len() >= limits.max_items {
        return None;
    }
    state
        .bytes
        .checked_add(result_bytes)
        .filter(|bytes| *bytes <= limits.max_bytes)
}

fn relay_drain_watermark_crossed(
    state: &RelayMailboxState,
    next_bytes: usize,
    limits: &RelayMailboxInner,
) -> bool {
    (state.queue.len() < limits.wake_items
        && state
            .queue
            .len()
            .checked_add(1)
            .is_some_and(|items| items >= limits.wake_items))
        || (state.bytes < limits.wake_bytes && next_bytes >= limits.wake_bytes)
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
