mod block_assembler;
mod client;
mod config;
mod miner;

pub use crate::block_assembler::{BlockAssembler, BlockAssemblerController};
pub use crate::client::Client;
pub use crate::config::Config;
pub use crate::miner::Miner;
use ckb_util::RwLock;
use jsonrpc_types::BlockTemplate;
use std::sync::Arc;

pub type Work = Arc<RwLock<Option<BlockTemplate>>>;
