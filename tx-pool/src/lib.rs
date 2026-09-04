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
//! `TxPoolAuthority` is the sole owner of transaction membership, lifecycle,
//! resource charge, indexes and the paired chain view. Validation produces
//! typed evidence outside its short synchronous lock; read-only plans are
//! version checked and applied atomically under that owner. A committed apply
//! emits immutable effects, and callbacks, relay I/O, cache publication and
//! persistence execute only after the authority lock has been released.
//!
//! Detached-chain replay is ordinary charged Recovery admission. Capacity
//! waits own no transaction state, no authority lock crosses `.await`, and
//! stale work is rejected by its typed generation, owner and chain-view
//! evidence rather than by lock timing.
//!
//! Block-template publication co-locates a content revision and reset epoch
//! with the current template. Full rebuilds and reset preparation run without
//! serializing partial work: full publication deliberately wins over racing
//! partial revisions but is rejected if a newer reset epoch landed; reset
//! publication consumes its exact generation token. Partial updates
//! (`update_uncles`, `update_proposals` and `update_transactions`) remain
//! concurrent and use a revision-checked optimistic swap. Every successful
//! full/reset replacement reissues all three level-triggered delta generations,
//! so a partial update which landed just before the replacement is retried
//! instead of lost.
//!
//! Input, policy, capacity, cancellation and stale-work outcomes are typed
//! errors. A typed structural contradiction marks the generation ineligible
//! for persistence and requests controlled shutdown; legal transaction input
//! cannot construct that path. Panics are not used for rejection, rollback,
//! retry or ownership control. The outer Rust task guard only prevents an
//! unexpected unwind from being followed by persistence of an unaudited
//! generation; it does not catch, repair, or restart state.

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
