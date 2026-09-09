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
//! The private Store owns immutable transaction entries, dependency indexes,
//! charges and the paired chain view. Workers resolve and verify outside owner
//! guards. A commit locks only its semantic read/write footprint, checks that
//! the observed entries and chain view still match, and installs the complete
//! change with its reserved notification batch. Independent transactions and
//! shared read-only cell dependencies can commit concurrently.
//!
//! One publisher invokes callbacks, relay and other endpoints after the commit
//! guards open. One template driver builds outside guards and publishes after
//! checking the selected transactions and uncle source. Chain changes invalidate
//! earlier work; detached transactions re-enter the charged recovery queue.
//!
//! Resource pressure, rejection and stale work return ordinary errors. A
//! structural contradiction faults the generation, wakes blocked work and
//! prevents persistence. No authority guard crosses an await or external call.

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

pub use ckb_jsonrpc_types::BlockTemplate;
pub use component::entry::{TxEntry, TxEntrySnapshot};
pub use component::recent_reject::RecentReject;
pub use service::{
    LocalRemovalCompetingProgress, RemoteTxBatchOutcome, TxPoolController, TxPoolServiceBuilder,
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
