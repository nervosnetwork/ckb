use crate::{core::Capacity, packed::Byte32};

#[derive(Debug, Default)]
pub struct BlockReward {
    pub total: Capacity,
    pub primary: Capacity,
    pub secondary: Capacity,
    pub tx_fee: Capacity,
    pub proposal_reward: Capacity,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct BlockIssuance {
    pub primary: Capacity,
    pub secondary: Capacity,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct MinerReward {
    pub primary: Capacity,
    pub secondary: Capacity,
    pub committed: Capacity,
    pub proposal: Capacity,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct BlockEconomicState {
    pub issuance: BlockIssuance,
    pub miner_reward: MinerReward,
    pub txs_fee: Capacity,
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
