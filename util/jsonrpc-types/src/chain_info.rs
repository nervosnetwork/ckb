use crate::{AlertMessage, EpochNumber, Timestamp};
use ckb_types::U256;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug)]
pub struct ChainInfo {
    // network name
    pub chain: String,
    // median time for the current tip block
    pub median_time: Timestamp,
    // the current epoch number
    pub epoch: EpochNumber,
    // the current difficulty
    pub difficulty: U256,
    // estimate of whether this node is in InitialBlockDownload mode
    pub is_initial_block_download: bool,
    // any network and blockchain warnings
    pub alerts: Vec<AlertMessage>,
}
