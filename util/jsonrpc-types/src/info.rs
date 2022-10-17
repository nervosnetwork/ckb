use crate::{AlertMessage, EpochNumberWithFraction, Timestamp, EpochNumber, BlockNumber};
use serde::{Deserialize, Serialize};
use ckb_types::{U256, H256, prelude::*};

#[derive(Deserialize, Serialize, Debug)]
pub enum DeploymentPos {
    /// light client protocol
    LightClient,
}

#[derive(Deserialize, Serialize, Debug)]
pub enum DeploymentState {
    /// First state that each softfork starts.
    /// The 0 epoch is by definition in this state for each deployment.
    Defined,
    /// For epochs past the `start` epoch.
    Started,
    /// For one epoch after the first epoch period with STARTED epochs of
    /// which at least `threshold` has the associated bit set in `version`.
    LockedIn,
    /// For all epochs after the LOCKED_IN epoch.
    Active,
    /// For one epoch period past the `timeout_epoch`, if LOCKED_IN was not reached.
    Failed,
}

/// Chain information.
#[derive(Deserialize, Serialize, Debug)]
pub struct ChainInfo {
    pub hash: H256,
    pub epoch: EpochNumber,
    pub deployments: BtreeMap<DeploymentPos, DeploymentInfo>
}

#[derive(Deserialize, Serialize, Debug)]
pub struct DeploymentInfo {
    pub bit: u8,
    pub start: EpochNumber,
    pub timeout: EpochNumber,
    pub min_activation_epoch: EpochNumber,
    pub state: DeploymentState,
}


/// Chain information.
#[derive(Deserialize, Serialize, Debug)]
pub struct ChainInfo {
    /// The network name.
    ///
    /// Examples:
    ///
    /// * "ckb" - Mirana the mainnet.
    /// * "ckb_testnet" - Pudge the testnet.
    pub chain: String,
    /// The median time of the last 37 blocks, including the tip block.
    pub median_time: Timestamp,
    /// The epoch information of tip block in the chain.
    pub epoch: EpochNumberWithFraction,
    /// Current difficulty.
    ///
    /// Decoded from the epoch `compact_target`.
    pub difficulty: U256,
    /// Whether the local node is in IBD, Initial Block Download.
    ///
    /// When a node starts and its chain tip timestamp is far behind the wall clock, it will enter
    /// the IBD until it catches up the synchronization.
    ///
    /// During IBD, the local node only synchronizes the chain with one selected remote node and
    /// stops responding the most P2P requests.
    pub is_initial_block_download: bool,
    /// Active alerts stored in the local node.
    pub alerts: Vec<AlertMessage>,
}
