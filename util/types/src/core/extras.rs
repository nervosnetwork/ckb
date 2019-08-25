use crate::{
    core::{BlockNumber, Capacity, EpochNumber},
    packed,
    prelude::*,
    H256, U256,
};
use failure::Error as FailureError;

#[derive(Clone, PartialEq, Default, Debug)]
pub struct BlockExt {
    pub received_at: u64,
    pub total_difficulty: U256,
    pub total_uncles_count: u64,
    pub verified: Option<bool>,
    pub txs_fees: Vec<Capacity>,
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct TransactionInfo {
    // Block hash
    pub block_hash: H256,
    pub block_number: BlockNumber,
    pub block_epoch: EpochNumber,
    // Index in the block
    pub index: usize,
}

impl TransactionInfo {
    pub fn key(&self) -> packed::TransactionKey {
        packed::TransactionKey::new_builder()
            .block_hash(self.block_hash.pack())
            .index(self.index.pack())
            .build()
    }

    pub fn new(
        block_number: BlockNumber,
        block_epoch: EpochNumber,
        block_hash: H256,
        index: usize,
    ) -> Self {
        TransactionInfo {
            block_number,
            block_epoch,
            block_hash,
            index,
        }
    }

    pub fn is_cellbase(&self) -> bool {
        self.index == 0
    }
}

#[derive(Clone, Eq, PartialEq, Debug, Default)]
pub struct EpochExt {
    pub(crate) number: EpochNumber,
    pub(crate) block_reward: Capacity,
    pub(crate) remainder_reward: Capacity,
    pub(crate) previous_epoch_hash_rate: U256,
    pub(crate) last_block_hash_in_previous_epoch: H256,
    pub(crate) start_number: BlockNumber,
    pub(crate) length: BlockNumber,
    pub(crate) difficulty: U256,
}

impl EpochExt {
    pub fn number(&self) -> u64 {
        self.number
    }

    pub fn block_reward(&self, number: BlockNumber) -> Result<Capacity, FailureError> {
        if number >= self.start_number()
            && number < self.start_number() + self.remainder_reward.as_u64()
        {
            self.block_reward
                .safe_add(Capacity::one())
                .map_err(Into::into)
        } else {
            Ok(self.block_reward)
        }
    }

    pub fn base_block_reward(&self) -> &Capacity {
        &self.block_reward
    }

    pub fn is_genesis(&self) -> bool {
        0 == self.number
    }

    pub fn start_number(&self) -> BlockNumber {
        self.start_number
    }

    pub fn length(&self) -> BlockNumber {
        self.length
    }

    pub fn set_length(&mut self, length: BlockNumber) {
        self.length = length;
    }

    pub fn set_difficulty(&mut self, difficulty: U256) {
        self.difficulty = difficulty;
    }

    pub fn difficulty(&self) -> &U256 {
        &self.difficulty
    }

    pub fn remainder_reward(&self) -> &Capacity {
        &self.remainder_reward
    }

    pub fn last_block_hash_in_previous_epoch(&self) -> &H256 {
        &self.last_block_hash_in_previous_epoch
    }

    pub fn previous_epoch_hash_rate(&self) -> &U256 {
        &self.previous_epoch_hash_rate
    }

    pub fn set_previous_epoch_hash_rate(&mut self, hash_rate: U256) {
        self.previous_epoch_hash_rate = hash_rate
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        number: u64,
        block_reward: Capacity,
        remainder_reward: Capacity,
        previous_epoch_hash_rate: U256,
        last_block_hash_in_previous_epoch: H256,
        start_number: BlockNumber,
        length: BlockNumber,
        difficulty: U256,
    ) -> EpochExt {
        EpochExt {
            number,
            block_reward,
            remainder_reward,
            previous_epoch_hash_rate,
            last_block_hash_in_previous_epoch,
            start_number,
            length,
            difficulty,
        }
    }

    pub fn destruct(
        self,
    ) -> (
        u64,
        Capacity,
        Capacity,
        U256,
        H256,
        BlockNumber,
        BlockNumber,
        U256,
    ) {
        let EpochExt {
            number,
            block_reward,
            remainder_reward,
            previous_epoch_hash_rate,
            last_block_hash_in_previous_epoch,
            start_number,
            length,
            difficulty,
        } = self;
        (
            number,
            block_reward,
            remainder_reward,
            previous_epoch_hash_rate,
            last_block_hash_in_previous_epoch,
            start_number,
            length,
            difficulty,
        )
    }
}
