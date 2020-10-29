use crate::{core::Capacity, packed::Byte32};

/// TODO(doc): @yangby-cryptape
#[derive(Debug, Default)]
pub struct BlockReward {
    /// TODO(doc): @yangby-cryptape
    pub total: Capacity,
    /// TODO(doc): @yangby-cryptape
    pub primary: Capacity,
    /// TODO(doc): @yangby-cryptape
    pub secondary: Capacity,
    /// TODO(doc): @yangby-cryptape
    pub tx_fee: Capacity,
    /// TODO(doc): @yangby-cryptape
    pub proposal_reward: Capacity,
}

/// TODO(doc): @yangby-cryptape
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BlockIssuance {
    /// TODO(doc): @yangby-cryptape
    pub primary: Capacity,
    /// TODO(doc): @yangby-cryptape
    pub secondary: Capacity,
}

/// TODO(doc): @yangby-cryptape
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MinerReward {
    /// TODO(doc): @yangby-cryptape
    pub primary: Capacity,
    /// TODO(doc): @yangby-cryptape
    pub secondary: Capacity,
    /// TODO(doc): @yangby-cryptape
    pub committed: Capacity,
    /// TODO(doc): @yangby-cryptape
    pub proposal: Capacity,
}

/// TODO(doc): @yangby-cryptape
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BlockEconomicState {
    /// TODO(doc): @yangby-cryptape
    pub issuance: BlockIssuance,
    /// TODO(doc): @yangby-cryptape
    pub miner_reward: MinerReward,
    /// TODO(doc): @yangby-cryptape
    pub txs_fee: Capacity,
    /// TODO(doc): @yangby-cryptape
    pub finalized_at: Byte32,
}

impl From<BlockReward> for MinerReward {
    fn from(reward: BlockReward) -> Self {
        Self {
            primary: reward.primary,
            secondary: reward.secondary,
            committed: reward.tx_fee,
            proposal: reward.proposal_reward,
        }
    }
}
