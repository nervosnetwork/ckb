//! CKB Tx-pool stores transactions, which is designed for CKB
//! [Two-Step-Transaction-Confirmation](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0020-ckb-consensus-protocol/0020-ckb-consensus-protocol.md#Two-Step-Transaction-Confirmation)
//! mechanism

#![cfg_attr(
    not(test),
    deny(
        clippy::arithmetic_side_effects,
        clippy::await_holding_lock,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unreachable,
        clippy::unwrap_used
    )
)]
//!
//! # Authority and lock hierarchy
//!
//! The private `Store` owns immutable transaction entries, derived indexes,
//! inline account budgets, `RankedPriorities`, and the paired chain view inside
//! one short-hold `RwLock<AuthorityState>` guarded by `ReorgGate` priority
//! preemption. Workers resolve, verify through the 64-shard
//! `TxVerificationCache`, and compact payloads outside authority locks; idle
//! `Verify` workers fuse `Resolve -> Verify -> Accept` directly when no
//! resolved backlog is queued, local RPCs fuse resolution with synchronous
//! verification, and network/proposal batches commit fresh prefixes in chunks.
//! A commit validates off-lock premises, evaluates membership/eviction policy
//! directly on live state, preflights invariants, reserves its ordered `Outbox`
//! batch, and commits atomically under a strict two-level lock order (`state`
//! above leaf bookkeeping mutexes).
//!
//! Reconciliation runs off-lock block preparation and read-phase parallel
//! planning without blocking concurrent RPC reads, then cuts over atomically
//! through `Store::commit_reorg` (escalating via `ReorgGate` if ordinary writes
//! race with the optimistic read phase). One publisher invokes callbacks,
//! relay, recent-reject, and estimator endpoints strictly after the commit lock
//! opens, while local submissions and chain transitions cooperatively assist
//! publication. One template driver builds outside locks, pre-converts
//! `JsonBlockTemplate`, and publishes after O(1) removal-revision or
//! selected-owner validation. `TxPoolController` executes reads and mutations
//! directly against `Weak<Pool>` under `RequestGate` backpressure and drain
//! control, reserving a single ordered `ChainControl` channel for causal chain
//! and administration transitions. Chain changes invalidate earlier work;
//! detached transactions re-enter the charged recovery queue.
//!
//! Resource pressure, rejection, and stale work return ordinary errors. A
//! structural contradiction faults the generation, wakes blocked work, and
//! prevents persistence. No authority lock crosses an await, I/O, VM execution,
//! callback, or displaced-payload drop.

mod authority;
pub mod block_assembler;
mod callback;
mod component;
pub(crate) mod constants;
mod dependency_sort;
pub mod error;
mod metrics;
mod network;
mod persisted;
pub mod service;
mod util;
mod verification;

#[cfg(feature = "internal")]
#[path = "tests/blocking_service.rs"]
pub mod internal_test_support;

#[cfg(test)]
#[path = "tests/support.rs"]
pub(crate) mod test_support;

#[cfg(feature = "packing-bench")]
#[doc(hidden)]
pub use authority::packing_bench;

pub use ckb_jsonrpc_types::BlockTemplate;
pub use component::entry::{TxEntry, TxEntrySnapshot};
pub use component::recent_reject::RecentReject;
pub use service::{
    LocalRemovalCompetingProgress, RemoteTxBatchOutcome, TxPoolController, TxPoolInputSnapshot,
    TxPoolServiceBuilder,
};
pub use tokio::sync::RwLock as TokioRwLock;

/// Internal/test injection target retained for block-reconstruction fixtures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlugTarget {
    /// Inject the transaction into the pending pool.
    Pending,
    /// Inject the transaction into the proposed pool.
    Proposed,
}
