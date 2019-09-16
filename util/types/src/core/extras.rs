use crate::{
    core::{BlockNumber, Capacity, EpochNumber},
    packed,
    prelude::*,
    U256,
};
use ckb_error::Error;
use std::fmt;
use std::num::ParseIntError;
use std::str::FromStr;

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
    pub block_hash: packed::Byte32,
    pub block_number: BlockNumber,
    pub block_epoch: EpochNumber,
    // Index in the block
    pub index: usize,
}

impl TransactionInfo {
    pub fn key(&self) -> packed::TransactionKey {
        packed::TransactionKey::new_builder()
            .block_hash(self.block_hash.clone())
            .index(self.index.pack())
            .build()
    }

    pub fn new(
        block_number: BlockNumber,
        block_epoch: EpochNumber,
        block_hash: packed::Byte32,
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

    pub fn is_genesis(&self) -> bool {
        self.block_number == 0
    }
}

#[derive(Clone, Eq, PartialEq, Debug, Default)]
pub struct EpochExt {
    pub(crate) number: EpochNumber,
    pub(crate) base_block_reward: Capacity,
    pub(crate) remainder_reward: Capacity,
    pub(crate) previous_epoch_hash_rate: U256,
    pub(crate) last_block_hash_in_previous_epoch: packed::Byte32,
    pub(crate) start_number: BlockNumber,
    pub(crate) length: BlockNumber,
    pub(crate) difficulty: U256,
}

#[derive(Clone, Debug)]
pub struct EpochExtBuilder(pub(crate) EpochExt);

impl EpochExt {
    //
    // Simple Getters
    //

    pub fn number(&self) -> BlockNumber {
        self.number
    }

    pub fn base_block_reward(&self) -> &Capacity {
        &self.base_block_reward
    }

    pub fn remainder_reward(&self) -> &Capacity {
        &self.remainder_reward
    }

    pub fn previous_epoch_hash_rate(&self) -> &U256 {
        &self.previous_epoch_hash_rate
    }

    pub fn last_block_hash_in_previous_epoch(&self) -> packed::Byte32 {
        self.last_block_hash_in_previous_epoch.clone()
    }

    pub fn start_number(&self) -> BlockNumber {
        self.start_number
    }

    pub fn length(&self) -> BlockNumber {
        self.length
    }

    pub fn difficulty(&self) -> &U256 {
        &self.difficulty
    }

    //
    // Simple Setters
    //

    pub fn set_number(&mut self, number: BlockNumber) {
        self.number = number;
    }

    pub fn set_base_block_reward(&mut self, base_block_reward: Capacity) {
        self.base_block_reward = base_block_reward;
    }

    pub fn set_remainder_reward(&mut self, remainder_reward: Capacity) {
        self.remainder_reward = remainder_reward;
    }

    pub fn set_previous_epoch_hash_rate(&mut self, previous_epoch_hash_rate: U256) {
        self.previous_epoch_hash_rate = previous_epoch_hash_rate;
    }

    pub fn set_last_block_hash_in_previous_epoch(
        &mut self,
        last_block_hash_in_previous_epoch: packed::Byte32,
    ) {
        self.last_block_hash_in_previous_epoch = last_block_hash_in_previous_epoch;
    }

    pub fn set_start_number(&mut self, start_number: BlockNumber) {
        self.start_number = start_number;
    }

    pub fn set_length(&mut self, length: BlockNumber) {
        self.length = length;
    }

    pub fn set_difficulty(&mut self, difficulty: U256) {
        self.difficulty = difficulty;
    }

    //
    // Normal Methods
    //

    pub fn new_builder() -> EpochExtBuilder {
        EpochExtBuilder(EpochExt::default())
    }

    pub fn into_builder(self) -> EpochExtBuilder {
        EpochExtBuilder(self)
    }

    pub fn is_genesis(&self) -> bool {
        0 == self.number
    }

    pub fn block_reward(&self, number: BlockNumber) -> Result<Capacity, Error> {
        if number >= self.start_number()
            && number < self.start_number() + self.remainder_reward.as_u64()
        {
            self.base_block_reward
                .safe_add(Capacity::one())
                .map_err(Into::into)
        } else {
            Ok(self.base_block_reward)
        }
    }

    pub fn number_with_fraction(&self, number: BlockNumber) -> DetailedEpochNumber {
        debug_assert!(
            number >= self.start_number() && number < self.start_number() + self.length()
        );
        let fraction = DetailedEpochNumber::FRACTION_MAXIMUM_VALUE * (number - self.start_number())
            / self.length();
        DetailedEpochNumber::new(self.number(), fraction, self.length())
    }
}

impl EpochExtBuilder {
    //
    // Simple Setters
    //

    pub fn number(mut self, number: BlockNumber) -> Self {
        self.0.set_number(number);
        self
    }

    pub fn base_block_reward(mut self, base_block_reward: Capacity) -> Self {
        self.0.set_base_block_reward(base_block_reward);
        self
    }

    pub fn remainder_reward(mut self, remainder_reward: Capacity) -> Self {
        self.0.set_remainder_reward(remainder_reward);
        self
    }

    pub fn previous_epoch_hash_rate(mut self, previous_epoch_hash_rate: U256) -> Self {
        self.0
            .set_previous_epoch_hash_rate(previous_epoch_hash_rate);
        self
    }

    pub fn last_block_hash_in_previous_epoch(
        mut self,
        last_block_hash_in_previous_epoch: packed::Byte32,
    ) -> Self {
        self.0
            .set_last_block_hash_in_previous_epoch(last_block_hash_in_previous_epoch);
        self
    }

    pub fn start_number(mut self, start_number: BlockNumber) -> Self {
        self.0.set_start_number(start_number);
        self
    }

    pub fn length(mut self, length: BlockNumber) -> Self {
        self.0.set_length(length);
        self
    }

    pub fn difficulty(mut self, difficulty: U256) -> Self {
        self.0.set_difficulty(difficulty);
        self
    }

    //
    // Normal Methods
    //
    // The `new` methods are unnecessary. Creating `EpochExtBuilder` from `EpochExt`, it's enough.

    pub fn build(self) -> EpochExt {
        self.0
    }
}

#[derive(Debug, Default, Clone, Eq, PartialEq, Hash)]
pub struct DetailedEpochNumber(u64);

impl fmt::Display for DetailedEpochNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for DetailedEpochNumber {
    type Err = ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let v = u64::from_str(s)?;
        Ok(DetailedEpochNumber(v))
    }
}

impl DetailedEpochNumber {
    pub const NUMBER_BITS: usize = 32;
    pub const NUMBER_MAXIMUM_VALUE: u64 = (1u64 << Self::NUMBER_BITS);
    pub const NUMBER_MASK: u64 = (Self::NUMBER_MAXIMUM_VALUE - 1);
    pub const FRACTION_BITS: usize = 16;
    pub const FRACTION_MAXIMUM_VALUE: u64 = (1u64 << Self::FRACTION_BITS);
    pub const FRACTION_MASK: u64 = (Self::FRACTION_MAXIMUM_VALUE - 1);
    pub const NUMBER_WITH_FRACTION_BITS: usize = Self::NUMBER_BITS + Self::FRACTION_BITS;
    pub const NUMBER_WITH_FRACTION_MAXIMUM_VALUE: u64 = (1u64 << Self::NUMBER_WITH_FRACTION_BITS);
    pub const NUMBER_WITH_FRACTION_MASK: u64 = (Self::NUMBER_WITH_FRACTION_MAXIMUM_VALUE - 1);
    pub const LENGTH_BITS: usize = 16;
    pub const LENGTH_MAXIMUM_VALUE: u64 = (1u64 << Self::LENGTH_BITS);
    pub const LENGTH_MASK: u64 = (Self::LENGTH_MAXIMUM_VALUE - 1);

    pub fn new(number: u64, fraction: u64, length: u64) -> DetailedEpochNumber {
        debug_assert!(number < Self::NUMBER_MAXIMUM_VALUE);
        debug_assert!(fraction < Self::FRACTION_MAXIMUM_VALUE);
        debug_assert!(length < Self::LENGTH_MAXIMUM_VALUE);
        DetailedEpochNumber(
            (length << Self::NUMBER_WITH_FRACTION_BITS)
                | (number << Self::FRACTION_BITS)
                | fraction,
        )
    }

    pub fn number(&self) -> u64 {
        (self.0 >> Self::FRACTION_BITS) & Self::NUMBER_MASK
    }

    pub fn fraction(&self) -> u64 {
        self.0 & Self::FRACTION_MASK
    }

    pub fn length(&self) -> u64 {
        (self.0 >> Self::NUMBER_WITH_FRACTION_BITS) & Self::LENGTH_MASK
    }

    pub fn number_with_fraction(&self) -> u64 {
        self.0 & Self::NUMBER_WITH_FRACTION_MASK
    }

    pub fn full_value(&self) -> u64 {
        self.0
    }

    pub fn from_full_value(value: u64) -> Self {
        Self(value)
    }
}
