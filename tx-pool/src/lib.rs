//! CKB Tx-pool stores transactions,
//! design for CKB [Two-Step-Transaction-Confirmation](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0020-ckb-consensus-protocol/0020-ckb-consensus-protocol.md#Two-Step-Transaction-Confirmation) mechanism

mod block_assembler;
mod callback;
mod component;
pub mod error;
pub mod pool;
mod process;
pub mod service;

pub use component::entry::TxEntry;
pub use process::PlugTarget;
pub use service::{TxPoolController, TxPoolServiceBuilder};
pub use tokio::sync::RwLock as TokioRwLock;
