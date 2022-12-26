use crate::{AlertMessage, EpochNumber, EpochNumberWithFraction, Timestamp};
use ckb_types::{core::Ratio, H256, U256};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Deployment name
#[derive(Deserialize, Serialize, Debug, Ord, PartialOrd, Eq, PartialEq)]
pub enum DeploymentPos {
    /// Dummy
    Testdummy,
    /// light client protocol
    LightClient,
}

/// The possible softfork deployment state
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
pub struct DeploymentsInfo {
    /// requested block hash
    pub hash: H256,
    /// requested block epoch
    pub epoch: EpochNumber,
    /// deployments info
    pub deployments: BTreeMap<DeploymentPos, DeploymentInfo>,
}

/// An object containing various state info regarding deployments of consensus changes
#[derive(Deserialize, Serialize, Debug)]
pub struct DeploymentInfo {
    /// determines which bit in the `version` field of the block is to be used to signal the softfork lock-in and activation.
    /// It is chosen from the set {0,1,2,...,28}.
    pub bit: u8,
    /// specifies the first epoch in which the bit gains meaning.
    pub start: EpochNumber,
    /// specifies an epoch at which the miner signaling ends.
    /// Once this epoch has been reached,
    /// if the softfork has not yet locked_in (excluding this epoch block's bit state),
    /// the deployment is considered failed on all descendants of the block.
    pub timeout: EpochNumber,
    /// specifies the epoch at which the softfork is allowed to become active.
    pub min_activation_epoch: EpochNumber,
    /// the length in epochs of the signalling period
    pub period: EpochNumber,
    /// the ratio of blocks with the version bit set required to activate the feature
    pub threshold: Ratio,
    /// With each epoch and softfork, we associate a deployment state. The possible states are
    pub state: DeploymentState,
    /// The first epoch which the current state applies
    pub since: EpochNumber,
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
