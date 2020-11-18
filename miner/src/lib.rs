//! TODO(doc): @quake
mod client;
mod error;
mod miner;
mod worker;

pub use crate::client::Client;
pub use crate::error::Error;
pub use crate::miner::Miner;

use ckb_jsonrpc_types::BlockTemplate;
use ckb_types::packed::Block;
use std::convert::From;

/// TODO(doc): @quake
#[derive(Clone)]
pub struct Work {
    /// TODO(doc): @quake
    work_id: u64,
    /// TODO(doc): @quake
    block: Block,
}

impl From<BlockTemplate> for Work {
    fn from(block_template: BlockTemplate) -> Work {
        let work_id = block_template.work_id;
        let block: Block = block_template.into();

        Work {
            work_id: work_id.into(),
            block,
        }
    }
}
