use crate::{
    U256,
    core::{BlockNumber, Capacity, CapacityResult, Cycle, EpochNumber},
    packed,
    prelude::*,
};
use ckb_rational::RationalU256;
use std::cmp::Ordering;
use std::fmt;
use std::num::ParseIntError;
use std::str::FromStr;

/// Represents a block's additional information.
///
/// It is crucial to ensure that `txs_sizes` has one more element than `txs_fees`, and that `cycles` has the same length as `txs_fees`.
///
/// `BlockTxsVerifier::verify()` skips the first transaction (the cellbase) in the block. Therefore, `txs_sizes` must have a length equal to `txs_fees` length + 1.
///
/// Refer to: https://github.com/nervosnetwork/ckb/blob/44afc93cd88a1b52351831dce788d3023c52f37e/verification/contextual/src/contextual_block_verifier.rs#L455
///
/// Additionally, the `get_fee_rate_statistics` RPC function requires accurate `txs_sizes` and `txs_fees` data from `BlockExt`.
#[derive(Clone, PartialEq, Default, Debug, Eq)]
pub struct BlockExt {
    /// TODO(doc): @quake
    pub received_at: u64,
    /// TODO(doc): @quake
    pub total_difficulty: U256,
    /// TODO(doc): @quake
    pub total_uncles_count: u64,
    /// TODO(doc): @quake
    pub verified: Option<bool>,
    /// Transaction fees for each transaction except the cellbase.
    /// The length of `txs_fees` is equal to the length of `cycles`.
    pub txs_fees: Vec<Capacity>,
    /// Execution cycles for each transaction except the cellbase.
    /// The length of `cycles` is equal to the length of `txs_fees`.
    pub cycles: Option<Vec<Cycle>>,
    /// Sizes of each transaction including the cellbase.
    /// The length of `txs_sizes` is `txs_fees` length + 1.
    pub txs_sizes: Option<Vec<u64>>,
}

/// TODO(doc): @quake
#[derive(Clone, Eq, PartialEq, Debug)]
pub struct TransactionInfo {
    /// TODO(doc): @quake
    // Block hash
    pub block_hash: packed::Byte32,
    /// TODO(doc): @quake
    pub block_number: BlockNumber,
    /// TODO(doc): @quake
    pub block_epoch: EpochNumberWithFraction,
    /// TODO(doc): @quake
    // Index in the block
    pub index: usize,
}

impl TransactionInfo {
    /// TODO(doc): @quake
    pub fn key(&self) -> packed::TransactionKey {
        packed::TransactionKey::new_builder()
            .block_hash(self.block_hash.clone())
            .index(self.index.pack())
            .build()
    }

    /// TODO(doc): @quake
    pub fn new(
        block_number: BlockNumber,
        block_epoch: EpochNumberWithFraction,
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

    /// TODO(doc): @quake
    pub fn is_cellbase(&self) -> bool {
        self.index == 0
    }

    /// TODO(doc): @quake
    pub fn is_genesis(&self) -> bool {
        self.block_number == 0
    }
}

/// TODO(doc): @quake
#[derive(Clone, Eq, PartialEq, Debug, Default)]
pub struct EpochExt {
    pub(crate) number: EpochNumber,
    pub(crate) base_block_reward: Capacity,
    pub(crate) remainder_reward: Capacity,
    pub(crate) previous_epoch_hash_rate: U256,
    pub(crate) last_block_hash_in_previous_epoch: packed::Byte32,
    pub(crate) start_number: BlockNumber,
    pub(crate) length: BlockNumber,
    pub(crate) compact_target: u32,
}

#[derive(Clone, Debug)]
pub struct EpochExtBuilder(pub(crate) EpochExt);

impl EpochExt {
    //
    // Simple Getters
    //

    /// TODO(doc): @quake
    pub fn number(&self) -> EpochNumber {
        self.number
    }

    /// TODO(doc): @quake
    pub fn primary_reward(&self) -> Capacity {
        Capacity::shannons(
            self.base_block_reward.as_u64() * self.length + self.remainder_reward.as_u64(),
        )
    }
    /// TODO(doc): @quake
    pub fn base_block_reward(&self) -> &Capacity {
        &self.base_block_reward
    }

    /// TODO(doc): @quake
    pub fn remainder_reward(&self) -> &Capacity {
        &self.remainder_reward
    }

    /// TODO(doc): @quake
    pub fn previous_epoch_hash_rate(&self) -> &U256 {
        &self.previous_epoch_hash_rate
    }

    /// TODO(doc): @quake
    pub fn last_block_hash_in_previous_epoch(&self) -> packed::Byte32 {
        self.last_block_hash_in_previous_epoch.clone()
    }

    /// TODO(doc): @quake
    pub fn start_number(&self) -> BlockNumber {
        self.start_number
    }

    /// TODO(doc): @quake
    pub fn length(&self) -> BlockNumber {
        self.length
    }

    /// TODO(doc): @quake
    pub fn compact_target(&self) -> u32 {
        self.compact_target
    }

    //
    // Simple Setters
    //

    /// TODO(doc): @quake
    pub fn set_number(&mut self, number: BlockNumber) {
        self.number = number;
    }

    /// TODO(doc): @quake
    pub fn set_base_block_reward(&mut self, base_block_reward: Capacity) {
        self.base_block_reward = base_block_reward;
    }

    /// TODO(doc): @quake
    pub fn set_remainder_reward(&mut self, remainder_reward: Capacity) {
        self.remainder_reward = remainder_reward;
    }

    /// TODO(doc): @quake
    pub fn set_previous_epoch_hash_rate(&mut self, previous_epoch_hash_rate: U256) {
        self.previous_epoch_hash_rate = previous_epoch_hash_rate;
    }

    /// TODO(doc): @quake
    pub fn set_last_block_hash_in_previous_epoch(
        &mut self,
        last_block_hash_in_previous_epoch: packed::Byte32,
    ) {
        self.last_block_hash_in_previous_epoch = last_block_hash_in_previous_epoch;
    }

    /// TODO(doc): @quake
    pub fn set_start_number(&mut self, start_number: BlockNumber) {
        self.start_number = start_number;
    }

    /// TODO(doc): @quake
    pub fn set_length(&mut self, length: BlockNumber) {
        self.length = length;
    }

    /// TODO(doc): @quake
    pub fn set_primary_reward(&mut self, primary_reward: Capacity) {
        let primary_reward_u64 = primary_reward.as_u64();
        self.base_block_reward = Capacity::shannons(primary_reward_u64 / self.length);
        self.remainder_reward = Capacity::shannons(primary_reward_u64 % self.length);
    }

    /// TODO(doc): @quake
    pub fn set_compact_target(&mut self, compact_target: u32) {
        self.compact_target = compact_target;
    }

    //
    // Normal Methods
    //

    /// TODO(doc): @quake
    pub fn new_builder() -> EpochExtBuilder {
        EpochExtBuilder(EpochExt::default())
    }

    /// TODO(doc): @quake
    pub fn into_builder(self) -> EpochExtBuilder {
        EpochExtBuilder(self)
    }

    /// TODO(doc): @quake
    pub fn is_genesis(&self) -> bool {
        0 == self.number
    }

    /// TODO(doc): @quake
    pub fn block_reward(&self, number: BlockNumber) -> CapacityResult<Capacity> {
        if number >= self.start_number()
            && number < self.start_number() + self.remainder_reward.as_u64()
        {
            self.base_block_reward.safe_add(Capacity::one())
        } else {
            Ok(self.base_block_reward)
        }
    }

    /// TODO(doc): @quake
    pub fn number_with_fraction(&self, number: BlockNumber) -> EpochNumberWithFraction {
        debug_assert!(
            number >= self.start_number() && number < self.start_number() + self.length()
        );
        EpochNumberWithFraction::new(self.number(), number - self.start_number(), self.length())
    }

    // We name this issuance since it covers multiple parts: block reward,
    // NervosDAO issuance as well as treasury part.
    /// TODO(doc): @quake
    pub fn secondary_block_issuance(
        &self,
        block_number: BlockNumber,
        secondary_epoch_issuance: Capacity,
    ) -> CapacityResult<Capacity> {
        let mut g2 = Capacity::shannons(secondary_epoch_issuance.as_u64() / self.length());
        let remainder = secondary_epoch_issuance.as_u64() % self.length();
        if block_number >= self.start_number() && block_number < self.start_number() + remainder {
            g2 = g2.safe_add(Capacity::one())?;
        }
        Ok(g2)
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

    pub fn compact_target(mut self, compact_target: u32) -> Self {
        self.0.set_compact_target(compact_target);
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

/// Represents an epoch number with a fraction unit, it can be
/// used to accurately represent the position for a block within
/// an epoch.
// Don't derive `Default` trait:
// - If we set default inner value to 0, it would panic when call `to_rational()`
// - But when uses it as an increment, "length == 0" is allowed, it's a valid default value.
// So, use `new()` or `new_unchecked()` to construct the instance depends on the context.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct EpochNumberWithFraction(u64);

impl fmt::Display for EpochNumberWithFraction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            write!(f, "{}({}/{})", self.number(), self.index(), self.length())
        } else {
            write!(
                f,
                "Epoch {{ number: {}, index: {}, length: {} }}",
                self.number(),
                self.index(),
                self.length()
            )
        }
    }
}

impl FromStr for EpochNumberWithFraction {
    type Err = ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let v = u64::from_str(s)?;
        Ok(EpochNumberWithFraction(v))
    }
}

impl PartialOrd for EpochNumberWithFraction {
    fn partial_cmp(&self, other: &EpochNumberWithFraction) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EpochNumberWithFraction {
    fn cmp(&self, other: &EpochNumberWithFraction) -> Ordering {
        match self.number().cmp(&other.number()) {
            ord @ Ordering::Less | ord @ Ordering::Greater => ord,
            _ => {
                let a = self.index() * other.length();
                let b = other.index() * self.length();
                a.cmp(&b)
            }
        }
    }
}

impl EpochNumberWithFraction {
    /// TODO(doc): @quake
    pub const NUMBER_OFFSET: usize = 0;
    /// TODO(doc): @quake
    pub const NUMBER_BITS: usize = 24;
    /// TODO(doc): @quake
    pub const NUMBER_MAXIMUM_VALUE: u64 = (1u64 << Self::NUMBER_BITS);
    /// TODO(doc): @quake
    pub const NUMBER_MASK: u64 = (Self::NUMBER_MAXIMUM_VALUE - 1);
    /// TODO(doc): @quake
    pub const INDEX_OFFSET: usize = Self::NUMBER_BITS;
    /// TODO(doc): @quake
    pub const INDEX_BITS: usize = 16;
    /// TODO(doc): @quake
    pub const INDEX_MAXIMUM_VALUE: u64 = (1u64 << Self::INDEX_BITS);
    /// TODO(doc): @quake
    pub const INDEX_MASK: u64 = (Self::INDEX_MAXIMUM_VALUE - 1);
    /// TODO(doc): @quake
    pub const LENGTH_OFFSET: usize = Self::NUMBER_BITS + Self::INDEX_BITS;
    /// TODO(doc): @quake
    pub const LENGTH_BITS: usize = 16;
    /// TODO(doc): @quake
    pub const LENGTH_MAXIMUM_VALUE: u64 = (1u64 << Self::LENGTH_BITS);
    /// TODO(doc): @quake
    pub const LENGTH_MASK: u64 = (Self::LENGTH_MAXIMUM_VALUE - 1);

    /// TODO(doc): @quake
    pub fn new(number: u64, index: u64, length: u64) -> EpochNumberWithFraction {
        debug_assert!(number < Self::NUMBER_MAXIMUM_VALUE);
        debug_assert!(index < Self::INDEX_MAXIMUM_VALUE);
        debug_assert!(length < Self::LENGTH_MAXIMUM_VALUE);
        debug_assert!(length > 0);
        Self::new_unchecked(number, index, length)
    }

    /// TODO(doc): @quake
    pub const fn new_unchecked(number: u64, index: u64, length: u64) -> Self {
        EpochNumberWithFraction(
            (length << Self::LENGTH_OFFSET)
                | (index << Self::INDEX_OFFSET)
                | (number << Self::NUMBER_OFFSET),
        )
    }

    /// TODO(doc): @quake
    pub fn number(self) -> EpochNumber {
        (self.0 >> Self::NUMBER_OFFSET) & Self::NUMBER_MASK
    }

    /// TODO(doc): @quake
    pub fn index(self) -> u64 {
        (self.0 >> Self::INDEX_OFFSET) & Self::INDEX_MASK
    }

    /// TODO(doc): @quake
    pub fn length(self) -> u64 {
        (self.0 >> Self::LENGTH_OFFSET) & Self::LENGTH_MASK
    }

    /// TODO(doc): @quake
    pub const fn full_value(self) -> u64 {
        self.0
    }

    /// Estimate the floor limit of epoch number after N blocks.
    ///
    /// Since we couldn't know the length of next epoch before reach the next epoch,
    /// this function could only return `self.number()` or `self.number()+1`.
    pub fn minimum_epoch_number_after_n_blocks(self, n: BlockNumber) -> EpochNumber {
        let number = self.number();
        let length = self.length();
        let index = self.index();
        if index + n >= length {
            number + 1
        } else {
            number
        }
    }

    /// TODO(doc): @quake
    // One caveat here, is that if the user specifies a zero epoch length either
    // deliberately, or by accident, calling to_rational() after that might
    // result in a division by zero panic. To prevent that, this method would
    // automatically rewrite the value to epoch index 0 with epoch length to
    // prevent panics
    pub fn from_full_value(value: u64) -> Self {
        Self::from_full_value_unchecked(value).normalize()
    }

    /// Converts from an unsigned 64 bits number without checks.
    ///
    /// # Notice
    ///
    /// The `EpochNumberWithFraction` constructed by this method has a potential risk that when
    /// call `self.to_rational()` may lead to a panic if the user specifies a zero epoch length.
    pub fn from_full_value_unchecked(value: u64) -> Self {
        Self(value)
    }

    /// Prevents leading to a panic if the `EpochNumberWithFraction` is constructed without checks.
    pub fn normalize(self) -> Self {
        if self.length() == 0 {
            Self::new(self.number(), 0, 1)
        } else {
            self
        }
    }

    /// Converts the epoch to an unsigned 256 bits rational.
    ///
    /// # Panics
    ///
    /// Only genesis epoch's length could be zero, otherwise causes a division-by-zero panic.
    pub fn to_rational(self) -> RationalU256 {
        if self.0 == 0 {
            RationalU256::zero()
        } else {
            RationalU256::new(self.index().into(), self.length().into()) + U256::from(self.number())
        }
    }

    /// Check if current value is the genesis block.
    pub fn is_genesis(&self) -> bool {
        self.number() == 0 && self.index() == 0 && self.length() == 0
    }

    /// Check if current value is another value's successor.
    pub fn is_successor_of(self, predecessor: Self) -> bool {
        if predecessor.index() + 1 == predecessor.length() {
            self.number() == predecessor.number() + 1 && self.index() == 0
        } else {
            self.number() == predecessor.number()
                && self.index() == predecessor.index() + 1
                && self.length() == predecessor.length()
        }
    }

    /// Check the data format.
    ///
    /// The epoch length should be greater than zero.
    /// The epoch index should be less than the epoch length.
    pub fn is_well_formed(self) -> bool {
        self.length() > 0 && self.length() > self.index()
    }

    /// Check the data format as an increment.
    ///
    /// The epoch index should be less than the epoch length or both of them are zero.
    pub fn is_well_formed_increment(self) -> bool {
        self.length() > self.index() || (self.length() == 0 && self.index() == 0)
    }
}
