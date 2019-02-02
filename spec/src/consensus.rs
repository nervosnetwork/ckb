use ckb_core::block::{Block, BlockBuilder};
use ckb_core::header::HeaderBuilder;
use ckb_core::transaction::Capacity;
use ckb_core::{BlockNumber, Cycle, Version};
use ckb_pow::{Pow, PowEngine};
use numext_fixed_uint::U256;
use std::sync::Arc;

pub const DEFAULT_BLOCK_REWARD: Capacity = 5_000;
pub const MAX_UNCLE_LEN: usize = 2;
pub const MAX_UNCLE_AGE: usize = 6;
pub const TRANSACTION_PROPAGATION_TIME: BlockNumber = 1;
pub const TRANSACTION_PROPAGATION_TIMEOUT: BlockNumber = 10;
pub const CELLBASE_MATURITY: usize = 100;
// TODO: should adjust this value based on CKB average block time
pub const MEDIAN_TIME_BLOCK_COUNT: usize = 11;

//TODO：find best ORPHAN_RATE_TARGET
pub const ORPHAN_RATE_TARGET: f32 = 0.1;
pub const POW_TIME_SPAN: u64 = 12 * 60 * 60 * 1000; // 12 hours
pub const POW_SPACING: u64 = 15 * 1000; //15s

pub const MAX_BLOCK_CYCLES: Cycle = 100_000_000;
pub const MAX_BLOCK_BYTES: u64 = 10_000_000; // 10MB
pub const MAX_INCREMENT_OCCUPIED_CAPACITY: u64 = 2_000_000; // 2MB
pub const BLOCK_VERSION: u32 = 0;

#[derive(Clone, PartialEq, Debug)]
pub struct Consensus {
    pub id: String,
    pub genesis_block: Block,
    pub initial_block_reward: Capacity,
    pub max_uncles_age: usize,
    pub max_uncles_len: usize,
    pub orphan_rate_target: f32,
    pub pow_time_span: u64,
    pub pow_spacing: u64,
    pub transaction_propagation_time: BlockNumber,
    pub transaction_propagation_timeout: BlockNumber,
    pub pow: Pow,
    // For each input, if the referenced output transaction is cellbase,
    // it must have at least `cellbase_maturity` confirmations;
    // else reject this transaction.
    pub cellbase_maturity: usize,
    // This parameter indicates the count of past blocks used in the median time calculation
    pub median_time_block_count: usize,
    // Maximum cycles that all the scripts in all the commit transactions can take
    pub max_block_cycles: Cycle,
    // Maximum number of bytes to use for the entire block
    pub max_block_bytes: u64,
    // Maximum number of increment occupied capacity for each blocks
    pub max_increment_occupied_capacity: u64,
    // block version number supported
    pub block_version: Version,
}

// genesis difficulty should not be zero
impl Default for Consensus {
    fn default() -> Self {
        let genesis_block = BlockBuilder::default()
            .with_header_builder(HeaderBuilder::default().difficulty(U256::one()));

        Consensus {
            genesis_block,
            id: "main".to_owned(),
            max_uncles_age: MAX_UNCLE_AGE,
            max_uncles_len: MAX_UNCLE_LEN,
            initial_block_reward: DEFAULT_BLOCK_REWARD,
            orphan_rate_target: ORPHAN_RATE_TARGET,
            pow_time_span: POW_TIME_SPAN,
            pow_spacing: POW_SPACING,
            transaction_propagation_time: TRANSACTION_PROPAGATION_TIME,
            transaction_propagation_timeout: TRANSACTION_PROPAGATION_TIMEOUT,
            pow: Pow::Dummy,
            cellbase_maturity: CELLBASE_MATURITY,
            median_time_block_count: MEDIAN_TIME_BLOCK_COUNT,
            max_block_cycles: MAX_BLOCK_CYCLES,
            max_block_bytes: MAX_BLOCK_BYTES,
            max_increment_occupied_capacity: MAX_INCREMENT_OCCUPIED_CAPACITY,
            block_version: BLOCK_VERSION,
        }
    }
}

impl Consensus {
    pub fn set_id(mut self, id: String) -> Self {
        self.id = id;
        self
    }

    pub fn set_genesis_block(mut self, genesis_block: Block) -> Self {
        self.genesis_block = genesis_block;
        self
    }

    pub fn set_initial_block_reward(mut self, initial_block_reward: Capacity) -> Self {
        self.initial_block_reward = initial_block_reward;
        self
    }

    pub fn set_pow(mut self, pow: Pow) -> Self {
        self.pow = pow;
        self
    }

    pub fn set_max_block_cycles(mut self, max_block_cycles: Cycle) -> Self {
        self.max_block_cycles = max_block_cycles;
        self
    }

    pub fn genesis_block(&self) -> &Block {
        &self.genesis_block
    }

    pub fn max_uncles_len(&self) -> usize {
        self.max_uncles_len
    }

    pub fn max_uncles_age(&self) -> usize {
        self.max_uncles_age
    }

    pub fn min_difficulty(&self) -> &U256 {
        self.genesis_block.header().difficulty()
    }

    pub fn initial_block_reward(&self) -> Capacity {
        self.initial_block_reward
    }

    pub fn difficulty_adjustment_interval(&self) -> BlockNumber {
        self.pow_time_span / self.pow_spacing
    }

    pub fn orphan_rate_target(&self) -> f32 {
        self.orphan_rate_target
    }

    pub fn pow_engine(&self) -> Arc<dyn PowEngine> {
        self.pow.engine()
    }

    pub fn cellbase_maturity(&self) -> usize {
        self.cellbase_maturity
    }

    pub fn median_time_block_count(&self) -> usize {
        self.median_time_block_count
    }

    pub fn max_block_cycles(&self) -> Cycle {
        self.max_block_cycles
    }

    pub fn max_block_bytes(&self) -> u64 {
        self.max_block_bytes
    }

    pub fn max_increment_occupied_capacity(&self) -> u64 {
        self.max_increment_occupied_capacity
    }

    pub fn block_version(&self) -> Version {
        self.block_version
    }
}
