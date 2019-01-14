use ckb_core::{Cycle, Version};
use serde_derive::Deserialize;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Config {
    pub rpc_url: String,
    pub poll_interval: u64,
    pub cycles_limit: Cycle,
    pub bytes_limit: usize,
    pub max_version: Version,
}
