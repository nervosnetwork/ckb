use crate::{BlockNumber, Capacity};
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, PartialEq, Default, Debug)]
pub struct BlockExt {
    pub received_at: u64,
    pub total_difficulty: U256,
    pub total_uncles_count: u64,
    pub txs_verified: Option<bool>,
}

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Debug)]
pub struct TransactionAddress {
    // Block hash
    pub block_hash: H256,
    // Offset of block transaction in serialized bytes
    pub offset: usize,
    pub length: usize,
}

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Debug)]
pub struct EpochExt {
    pub(crate) number: u64,
    pub(crate) block_reward: Capacity,
    pub(crate) start: BlockNumber,
    pub(crate) length: BlockNumber,
    pub(crate) difficulty: U256,
    pub(crate) remainder_reward: Capacity,
}

impl EpochExt {
    pub fn number(&self) -> u64 {
        self.number
    }

    pub fn block_reward(&self) -> Capacity {
        self.block_reward
    }

    pub fn start(&self) -> BlockNumber {
        self.start
    }

    pub fn length(&self) -> BlockNumber {
        self.length
    }

    pub fn difficulty(&self) -> &U256 {
        &self.difficulty
    }

    pub fn remainder_reward(&self) -> &Capacity {
        &self.remainder_reward
    }

    pub fn new(
        number: u64,
        block_reward: Capacity,
        remainder_reward: Capacity,
        start: BlockNumber,
        length: BlockNumber,
        difficulty: U256,
    ) -> EpochExt {
        EpochExt {
            number,
            block_reward,
            start,
            length,
            difficulty,
            remainder_reward,
        }
    }
}
