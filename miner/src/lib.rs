mod block_assembler;
mod client;
mod config;
mod error;
mod miner;
mod worker;

pub use crate::block_assembler::{BlockAssembler, BlockAssemblerController};
pub use crate::client::Client;
pub use crate::config::{BlockAssemblerConfig, ClientConfig, MinerConfig, WorkerConfig};
pub use crate::error::Error;
pub use crate::miner::Miner;

use ckb_core::block::Block;

pub struct Work {
    work_id: u64,
    block: Block,
}
