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
//! # Lock hierarchy
//!
//! There are two executable transaction authorities: accepted membership in
//! `TxPool`, and all pre-pool lifecycle in `PrePoolKernel`. The latter
//! is protected by one short-held synchronous mutex and is never held across
//! `.await`. Any operation that needs both takes `tx_pool` first and then the
//! kernel; kernel-only code must never acquire `tx_pool`.
//!
//! Detached-chain replay is ordinary charged `ResolveQueued` ownership with a
//! trusted Recovery source in `PrePoolKernel`; no lock spans asynchronous
//! validation. Persistence
//! takes `tx_pool` read then the kernel, copies one immutable bounded v2
//! snapshot, releases both and serializes only file writers. A capacity wait
//! owns no state and occurs before authority locks. The mutation order is
//! `optional serial/work guard -> tx_pool -> PrePoolKernel -> EffectJournal`;
//! the journal lock is innermost and binds total Apply to its immutable batch.
//! Callback re-entry follows the ordinary dispatcher and lock order, so the
//! effect publisher cannot create a reverse edge. Callbacks, relay I/O, and
//! persistence I/O run only after state mutation has been journaled and no
//! state lock is held.
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
//! Combined membership reads take `tx_pool` and then inspect the pre-pool kernel
//! under its synchronous lock, matching the writer order and preventing a
//! visible handoff gap.
//! Input, policy, capacity, cancellation and stale-lease outcomes are typed
//! errors. A typed structural contradiction marks the generation ineligible
//! for persistence and requests controlled shutdown; legal transaction input
//! cannot construct that path. Panics are not used for rejection, rollback,
//! retry or ownership control. The outer Rust task guard only prevents an
//! unexpected unwind from being followed by persistence of an unaudited
//! generation; it does not catch, repair, or restart state.

#[cfg(feature = "internal")]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::await_holding_lock,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unreachable,
    clippy::unwrap_used
)]
pub mod benchmark;
pub mod block_assembler;
mod callback;
mod component;
pub(crate) mod constants;
pub mod error;
mod metrics;
mod network;
mod persisted;
pub mod pool;
mod pool_cell;
mod process;
mod resolved_tx;
pub mod service;
pub(crate) mod tx_source;
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
pub use pool::TxPool;
pub use process::PlugTarget;
pub use service::{TxPoolController, TxPoolServiceBuilder};
pub use tokio::sync::RwLock as TokioRwLock;
