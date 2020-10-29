//! TODO(doc): @zhangsoledad
mod block_assembler;
mod component;
pub mod error;
pub mod pool;
mod process;
pub mod service;

pub use component::entry::TxEntry;
pub use process::PlugTarget;
pub use service::{TxPoolController, TxPoolServiceBuilder};
pub use tokio::sync::RwLock as TokioRwLock;
