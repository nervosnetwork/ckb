//! CKB Tx-pool stores transactions, which is designed for CKB
//! [Two-Step-Transaction-Confirmation](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0020-ckb-consensus-protocol/0020-ckb-consensus-protocol.md#Two-Step-Transaction-Confirmation)
//! mechanism
//!
//! # Lock hierarchy
//!
//! There are two executable transaction authorities: accepted membership in
//! `TxPool`, and all pre-pool lifecycle in `PrePoolKernel`. The latter
//! is protected by one short-held synchronous mutex and is never held across
//! `.await`. Any operation that needs both takes `tx_pool` first and then the
//! kernel; kernel-only code must never acquire `tx_pool`.
//!
//! Detached-chain replay is ordinary charged `RecoveryRetained` ownership in
//! `PrePoolKernel`; no lock spans its asynchronous validation. Persistence
//! takes `tx_pool` read then the kernel, copies one immutable bounded v2
//! snapshot, releases both and serializes only file writers. A capacity wait
//! owns no state and occurs before authority locks. The mutation order is
//! `optional serial/work guard -> tx_pool -> PrePoolKernel -> EffectJournal`;
//! the journal lock is innermost and binds total Apply to its immutable batch.
//! Callback-originated mutations fail fast, so the effect publisher cannot
//! create a reverse edge. Callbacks, relay I/O, and persistence I/O run only
//! after state mutation has been journaled and no state lock is held.
//!
//! `block_assembler.template_lock` guards the current block template.
//! `update_full` and `reset_template` acquire `template_lock` first and then
//! read `tx_pool` so a concurrent `Reset` cannot swap the template while
//! the full update is in progress. Partial updates such as `update_uncles`,
//! `update_proposals` and `update_transactions` do not touch the template and
//! therefore do not acquire `template_lock`.
//!
//! Combined membership reads take `tx_pool` and then inspect the pre-pool kernel
//! under its synchronous lock, matching the writer order and preventing a
//! visible handoff gap.
//! An impossible authoritative Apply unwind exchanges both ephemeral
//! generations under the existing write boundary and cools only Remote
//! ingress; it never cancels the tx-pool service.

#[cfg(feature = "internal")]
pub mod benchmark;
pub mod block_assembler;
mod callback;
mod component;
pub(crate) mod constants;
pub mod error;
mod network;
mod persisted;
pub mod pool;
mod pool_cell;
mod process;
mod resolve_mgr;
mod resolved_tx;
pub mod service;
pub(crate) mod tx_source;
mod util;
mod verify_mgr;
mod worker;

pub use ckb_jsonrpc_types::BlockTemplate;
pub use component::entry::{TxEntry, TxEntrySnapshot};
pub use component::recent_reject::RecentReject;
pub use pool::TxPool;
pub use process::PlugTarget;
pub use service::{TxPoolController, TxPoolServiceBuilder};
pub use tokio::sync::RwLock as TokioRwLock;
