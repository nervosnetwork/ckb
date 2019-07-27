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

use ckb_core::block::{Block, BlockBuilder};
use ckb_jsonrpc_types::BlockTemplate;
use std::convert::From;

pub struct Work {
    work_id: u64,
    block: Block,
}

impl From<BlockTemplate> for Work {
    fn from(block_template: BlockTemplate) -> Work {
        let work_id = block_template.work_id.clone();
        let block: BlockBuilder = block_template.into();
        let block = block.build();

        Work {
            work_id: work_id.0,
            block,
        }
    }
}
