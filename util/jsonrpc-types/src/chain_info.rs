use crate::{AlertMessage, EpochNumber, Timestamp};
use ckb_types::U256;
use serde::{Deserialize, Serialize};

/// Chain information.
#[derive(Deserialize, Serialize, Debug)]
pub struct ChainInfo {
    /// The network name.
    ///
    /// Examples:
    ///
    /// * "ckb" - Lina the mainnet.
    /// * "ckb_testnet" - Aggron the testnet.
    pub chain: String,
    /// The median time of the last 37 blocks.
    pub median_time: Timestamp,
    /// Current epoch number.
    pub epoch: EpochNumber,
    /// Current difficulty.
    ///
    /// The 256-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON.
    pub difficulty: U256,
    /// Whether the local node is in IBD, Initial Block Download.
    ///
    /// When a node starts and its chain tip timestamp is far behind the wall clock, it will enter
    /// the IBD until it catches up the synchronization.
    ///
    /// During IBD, the local node only synchronize the chain with one selected remote node and
    /// stop responding most P2P requests.
    pub is_initial_block_download: bool,
    /// Active alerts stored in the local node.
    pub alerts: Vec<AlertMessage>,
}
