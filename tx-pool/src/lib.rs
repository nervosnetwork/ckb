//! CKB Tx-pool stores transactions, which is designed for CKB
//! [Two-Step-Transaction-Confirmation](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0020-ckb-consensus-protocol/0020-ckb-consensus-protocol.md#Two-Step-Transaction-Confirmation)
//! mechanism

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
