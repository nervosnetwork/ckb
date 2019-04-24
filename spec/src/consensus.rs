use ckb_core::block::{Block, BlockBuilder};
use ckb_core::extras::EpochExt;
use ckb_core::header::HeaderBuilder;
use ckb_core::{BlockNumber, Capacity, Cycle, Version};
use ckb_pow::{Pow, PowEngine};
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::sync::Arc;

pub(crate) const DEFAULT_EPOCH_REWARD: u64 = 5_000_000;
pub(crate) const GENESIS_EPOCH_LENGTH: u64 = 1_000;
pub(crate) const GENESIS_EPOCH_REWARD: u64 = DEFAULT_EPOCH_REWARD / GENESIS_EPOCH_LENGTH;
pub(crate) const MAX_UNCLE_NUM: usize = 2;
pub(crate) const MAX_UNCLE_AGE: usize = 6;
pub(crate) const TX_PROPOSAL_WINDOW: ProposalWindow = ProposalWindow(2, 10);
pub(crate) const CELLBASE_MATURITY: BlockNumber = 100;
// TODO: should adjust this value based on CKB average block time
pub(crate) const MEDIAN_TIME_BLOCK_COUNT: usize = 11;

//TODO：find best ORPHAN_RATE_TARGET
pub(crate) const ORPHAN_RATE_TARGET_RECIP: u64 = 10;

const MAX_BLOCK_INTERVAL: u64 = 25 * 6 * 1000; // 2.5min
const MIN_BLOCK_INTERVAL: u64 = 5 * 1000; // 5s
pub(crate) const EPOCH_DURATION: u64 = 2 * 60 * 60 * 1000; // 1hour
pub(crate) const MAX_EPOCH_LENGTH: u64 = EPOCH_DURATION / MIN_BLOCK_INTERVAL; // 1440
pub(crate) const MIN_EPOCH_LENGTH: u64 = EPOCH_DURATION / MAX_BLOCK_INTERVAL; // 48

pub(crate) const MAX_BLOCK_CYCLES: Cycle = 20_000_000_000;
pub(crate) const MAX_BLOCK_BYTES: u64 = 2_000_000; // 2mb
pub(crate) const MAX_BLOCK_PROPOSALS_LIMIT: u64 = 6_000;
pub(crate) const BLOCK_VERSION: u32 = 0;

pub(crate) const MAX_TRANSACTION_MEMORY_BYTES: u64 = 10_000_000; // 10mb

#[derive(Clone, PartialEq, Debug, Eq, Copy)]
pub struct ProposalWindow(pub BlockNumber, pub BlockNumber);

impl ProposalWindow {
    pub fn end(&self) -> BlockNumber {
        self.0
    }

    pub fn start(&self) -> BlockNumber {
        self.1
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Consensus {
    pub id: String,
    pub genesis_block: Block,
    pub genesis_hash: H256,
    pub epoch_reward: Capacity,
    pub max_uncles_age: usize,
    pub max_uncles_num: usize,
    pub orphan_rate_target_recip: u64,
    pub epoch_duration: u64,
    pub tx_proposal_window: ProposalWindow,
    pub pow: Pow,
    // For each input, if the referenced output transaction is cellbase,
    // it must have at least `cellbase_maturity` confirmations;
    // else reject this transaction.
    pub cellbase_maturity: BlockNumber,
    // This parameter indicates the count of past blocks used in the median time calculation
    pub median_time_block_count: usize,
    // Maximum cycles that all the scripts in all the commit transactions can take
    pub max_block_cycles: Cycle,
    // Maximum number of bytes to use for the entire block
    pub max_block_bytes: u64,
    // Maximum number of memory bytes to verify a transaction
    pub max_transaction_memory_bytes: u64,
    // block version number supported
    pub block_version: Version,
    // block version number supported
    pub max_block_proposals_limit: u64,
    pub genesis_epoch_ext: EpochExt,
}

// genesis difficulty should not be zero
impl Default for Consensus {
    fn default() -> Self {
        let genesis_block = BlockBuilder::default()
            .with_header_builder(HeaderBuilder::default().difficulty(U256::one()));

        let genesis_epoch_ext = EpochExt::new(
            0, // number
            Capacity::shannons(GENESIS_EPOCH_REWARD), // block_reward
            Capacity::shannons(0), // remainder_reward
            0, // start
            1000, // length
            genesis_block.header().difficulty().clone() // difficulty,
        );

        Consensus {
            genesis_hash: genesis_block.header().hash(),
            genesis_block,
            id: "main".to_owned(),
            max_uncles_age: MAX_UNCLE_AGE,
            max_uncles_num: MAX_UNCLE_NUM,
            epoch_reward: Capacity::shannons(DEFAULT_EPOCH_REWARD),
            orphan_rate_target_recip: ORPHAN_RATE_TARGET_RECIP,
            epoch_duration: EPOCH_DURATION,
            tx_proposal_window: TX_PROPOSAL_WINDOW,
            pow: Pow::Dummy(Default::default()),
            cellbase_maturity: CELLBASE_MATURITY,
            median_time_block_count: MEDIAN_TIME_BLOCK_COUNT,
            max_block_cycles: MAX_BLOCK_CYCLES,
            max_block_bytes: MAX_BLOCK_BYTES,
            max_transaction_memory_bytes: MAX_TRANSACTION_MEMORY_BYTES,
            genesis_epoch_ext,
            block_version: BLOCK_VERSION,
            max_block_proposals_limit: MAX_BLOCK_PROPOSALS_LIMIT,
        }
    }
}

impl Consensus {
    pub fn set_id(mut self, id: String) -> Self {
        self.id = id;
        self
    }

    pub fn set_genesis_block(mut self, genesis_block: Block) -> Self {
        self.genesis_hash = genesis_block.header().hash();
        self.genesis_block = genesis_block;
        self
    }

    pub fn set_epoch_reward(mut self, epoch_reward: Capacity) -> Self {
        self.epoch_reward = epoch_reward;
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

    pub fn set_cellbase_maturity(mut self, cellbase_maturity: BlockNumber) -> Self {
        self.cellbase_maturity = cellbase_maturity;
        self
    }

    pub fn genesis_block(&self) -> &Block {
        &self.genesis_block
    }

    pub fn genesis_hash(&self) -> &H256 {
        &self.genesis_hash
    }

    pub fn max_uncles_num(&self) -> usize {
        self.max_uncles_num
    }

    pub fn max_uncles_age(&self) -> usize {
        self.max_uncles_age
    }

    pub fn min_difficulty(&self) -> &U256 {
        self.genesis_block.header().difficulty()
    }

    pub fn epoch_reward(&self) -> Capacity {
        self.epoch_reward
    }

    pub fn epoch_duration(&self) -> u64 {
        self.epoch_duration
    }

    pub fn genesis_epoch_ext(&self) -> &EpochExt {
        &self.genesis_epoch_ext
    }

    pub fn max_epoch_length(&self) -> BlockNumber {
        MAX_EPOCH_LENGTH
    }

    pub fn min_epoch_length(&self) -> BlockNumber {
        MIN_EPOCH_LENGTH
    }

    pub fn orphan_rate_target_recip(&self) -> u64 {
        self.orphan_rate_target_recip
    }

    pub fn pow_engine(&self) -> Arc<dyn PowEngine> {
        self.pow.engine()
    }

    pub fn cellbase_maturity(&self) -> BlockNumber {
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

    pub fn max_block_proposals_limit(&self) -> u64 {
        self.max_block_proposals_limit
    }

    pub fn max_transaction_memory_bytes(&self) -> u64 {
        self.max_transaction_memory_bytes
    }

    pub fn block_version(&self) -> Version {
        self.block_version
    }

    pub fn tx_proposal_window(&self) -> ProposalWindow {
        self.tx_proposal_window
    }
}
