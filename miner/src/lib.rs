mod block_assembler;
mod client;
mod config;
mod error;
mod miner;

pub use crate::block_assembler::{BlockAssembler, BlockAssemblerController};
pub use crate::client::Client;
pub use crate::config::{BlockAssemblerConfig, MinerConfig};
pub use crate::error::Error;
pub use crate::miner::Miner;
use ckb_util::Mutex;
use jsonrpc_types::BlockTemplate;
use std::sync::Arc;

pub type Work = Arc<Mutex<Option<BlockTemplate>>>;
