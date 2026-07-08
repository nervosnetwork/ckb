//! CKB Tx-pool stores transactions, which is designed for CKB
//! [Two-Step-Transaction-Confirmation](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0020-ckb-consensus-protocol/0020-ckb-consensus-protocol.md#Two-Step-Transaction-Confirmation)
//! mechanism
//!
//! # Lock hierarchy
//!
//! The pipeline and the main pool use several async locks. To avoid deadlocks,
//! acquire them in the following order whenever more than one is needed in the
//! same task:
//!
//! 1. `ordered_resolve_queue`
//! 2. `verify_queue`
//! 3. `rbf_candidates`
//! 4. `orphan`
//! 5. `block_assembler.template_lock`
//! 6. `tx_pool` (`update_full`/`reset_template` hold `template_lock` first,
//!    then read `tx_pool`; partial updates do not acquire `template_lock`)
//!
//! Read-only aggregations such as `info()` should acquire each lock in
//! isolation rather than holding several at once.

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
pub use component::entry::TxEntry;
pub use component::recent_reject::RecentReject;
pub use pool::TxPool;
pub use process::PlugTarget;
pub use service::{TxPoolController, TxPoolServiceBuilder};
pub use tokio::sync::RwLock as TokioRwLock;
