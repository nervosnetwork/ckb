use crate::EpochNumber;
use numext_fixed_uint::U256;
use serde_derive::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug)]
pub struct ChainInfo {
    // network name
    pub chain: String,
    // median time for the current tip block
    pub median_time: String,
    // the current epoch number
    pub epoch: EpochNumber,
    // the current difficulty
    pub difficulty: U256,
    // estimate of whether this node is in InitialBlockDownload mode
    pub is_initial_block_download: bool,
    // any network and blockchain warnings
    pub warnings: String,
}
