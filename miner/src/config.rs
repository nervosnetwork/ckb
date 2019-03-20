use ckb_core::{Cycle, Version};
use numext_fixed_hash::H256;
use serde_derive::Deserialize;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct MinerConfig {
    pub rpc_url: String,
    pub poll_interval: u64,
    pub cycles_limit: Cycle,
    pub bytes_limit: usize,
    pub max_version: Version,
    pub block_on_submit: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct BlockAssemblerConfig {
    pub reference: H256,
    pub args: Vec<Vec<u8>>,
}
