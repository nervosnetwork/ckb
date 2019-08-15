use crate::core::Capacity;

pub struct BlockReward {
    pub total: Capacity,
    pub primary: Capacity,
    pub secondary: Capacity,
    pub tx_fee: Capacity,
    pub proposal_reward: Capacity,
}
