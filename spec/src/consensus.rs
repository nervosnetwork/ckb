use ckb_core::block::{Block, BlockBuilder};
use ckb_core::extras::EpochExt;
use ckb_core::header::Header;
use ckb_core::header::HeaderBuilder;
use ckb_core::{capacity_bytes, BlockNumber, Capacity, Cycle, Version};
use ckb_pow::{Pow, PowEngine};
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::cmp;
use std::sync::Arc;

// TODO: add secondary reward for miner
pub(crate) const DEFAULT_SECONDARY_EPOCH_REWARD: Capacity = capacity_bytes!(50);
pub(crate) const DEFAULT_EPOCH_REWARD: Capacity = capacity_bytes!(5_000_000);
pub(crate) const MAX_UNCLE_NUM: usize = 2;
pub(crate) const TX_PROPOSAL_WINDOW: ProposalWindow = ProposalWindow(2, 10);
pub(crate) const CELLBASE_MATURITY: BlockNumber = 100;
// TODO: should adjust this value based on CKB average block time
pub(crate) const MEDIAN_TIME_BLOCK_COUNT: usize = 11;

//TODO：find best ORPHAN_RATE_TARGET
pub(crate) const ORPHAN_RATE_TARGET_RECIP: u64 = 20;

const MAX_BLOCK_INTERVAL: u64 = 60 * 1000; // 60s
const MIN_BLOCK_INTERVAL: u64 = 8 * 1000; // 8s
const TWO_IN_TWO_OUT_CYCLES: Cycle = 2_580_000;
pub(crate) const EPOCH_DURATION_TARGET: u64 = 8 * 60 * 60 * 1000; // 8 hours
pub(crate) const MAX_EPOCH_LENGTH: u64 = EPOCH_DURATION_TARGET / MIN_BLOCK_INTERVAL; // 3600
pub(crate) const MIN_EPOCH_LENGTH: u64 = EPOCH_DURATION_TARGET / MAX_BLOCK_INTERVAL; // 480
pub(crate) const GENESIS_EPOCH_LENGTH: u64 = 2_000;
pub(crate) const MAX_BLOCK_BYTES: u64 = 2_000_000; // 2mb
pub(crate) const MAX_BLOCK_CYCLES: u64 = TWO_IN_TWO_OUT_CYCLES * 200 * 8;
pub(crate) const MAX_BLOCK_PROPOSALS_LIMIT: u64 = 3_000;
pub(crate) const BLOCK_VERSION: u32 = 0;

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
    pub secondary_epoch_reward: Capacity,
    pub max_uncles_num: usize,
    pub orphan_rate_target_recip: u64,
    pub epoch_duration_target: u64,
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
    // block version number supported
    pub block_version: Version,
    // block version number supported
    pub max_block_proposals_limit: u64,
    pub genesis_epoch_ext: EpochExt,
}

// genesis difficulty should not be zero
impl Default for Consensus {
    fn default() -> Self {
        let genesis_block =
            BlockBuilder::from_header_builder(HeaderBuilder::default().difficulty(U256::one()))
                .build();

        let block_reward = Capacity::shannons(DEFAULT_EPOCH_REWARD.as_u64() / GENESIS_EPOCH_LENGTH);
        let remainder_reward =
            Capacity::shannons(DEFAULT_EPOCH_REWARD.as_u64() % GENESIS_EPOCH_LENGTH);

        let genesis_epoch_ext = EpochExt::new(
            0, // number
            block_reward,     // block_reward
            remainder_reward, // remainder_reward
            H256::zero(),
            0, // start
            GENESIS_EPOCH_LENGTH, // length
            genesis_block.header().difficulty().clone() // difficulty,
        );

        Consensus {
            genesis_hash: genesis_block.header().hash().to_owned(),
            genesis_block,
            id: "main".to_owned(),
            max_uncles_num: MAX_UNCLE_NUM,
            epoch_reward: DEFAULT_EPOCH_REWARD,
            orphan_rate_target_recip: ORPHAN_RATE_TARGET_RECIP,
            epoch_duration_target: EPOCH_DURATION_TARGET,
            secondary_epoch_reward: DEFAULT_SECONDARY_EPOCH_REWARD,
            tx_proposal_window: TX_PROPOSAL_WINDOW,
            pow: Pow::Dummy,
            cellbase_maturity: CELLBASE_MATURITY,
            median_time_block_count: MEDIAN_TIME_BLOCK_COUNT,
            max_block_cycles: MAX_BLOCK_CYCLES,
            max_block_bytes: MAX_BLOCK_BYTES,
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
        self.genesis_epoch_ext
            .set_difficulty(genesis_block.header().difficulty().clone());
        self.genesis_hash = genesis_block.header().hash().to_owned();
        self.genesis_block = genesis_block;
        self
    }

    pub fn set_genesis_epoch_ext(mut self, genesis_epoch_ext: EpochExt) -> Self {
        self.genesis_epoch_ext = genesis_epoch_ext;
        self
    }

    #[must_use]
    pub fn set_epoch_reward(mut self, epoch_reward: Capacity) -> Self {
        self.epoch_reward = epoch_reward;
        self
    }

    #[must_use]
    pub fn set_secondary_epoch_reward(mut self, secondary_epoch_reward: Capacity) -> Self {
        self.secondary_epoch_reward = secondary_epoch_reward;
        self
    }

    #[must_use]
    pub fn set_max_block_cycles(mut self, max_block_cycles: Cycle) -> Self {
        self.max_block_cycles = max_block_cycles;
        self
    }

    #[must_use]
    pub fn set_cellbase_maturity(mut self, cellbase_maturity: BlockNumber) -> Self {
        self.cellbase_maturity = cellbase_maturity;
        self
    }

    pub fn set_tx_proposal_window(mut self, proposal_window: ProposalWindow) -> Self {
        self.tx_proposal_window = proposal_window;
        self
    }

    pub fn set_pow(mut self, pow: Pow) -> Self {
        self.pow = pow;
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

    pub fn min_difficulty(&self) -> &U256 {
        self.genesis_block.header().difficulty()
    }

    pub fn epoch_reward(&self) -> Capacity {
        self.epoch_reward
    }

    pub fn epoch_duration_target(&self) -> u64 {
        self.epoch_duration_target
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

    pub fn secondary_epoch_reward(&self) -> Capacity {
        self.secondary_epoch_reward
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

    pub fn block_version(&self) -> Version {
        self.block_version
    }

    pub fn tx_proposal_window(&self) -> ProposalWindow {
        self.tx_proposal_window
    }

    pub fn revision_epoch_length(&self, raw: BlockNumber) -> BlockNumber {
        let max_length = self.max_epoch_length();
        let min_length = self.min_epoch_length();
        cmp::max(cmp::min(max_length, raw), min_length)
    }

    pub fn revision_epoch_difficulty(&self, last: U256, raw: U256) -> U256 {
        let min_difficulty = cmp::max(self.min_difficulty().clone(), &last / 2u64);
        let max_difficulty = last * 2u32;

        if raw > max_difficulty {
            return max_difficulty;
        }

        if raw < min_difficulty {
            return min_difficulty.clone();
        }
        raw
    }

    pub fn next_epoch_ext<A, B>(
        &self,
        last_epoch: &EpochExt,
        header: &Header,
        get_header: A,
        total_uncles_count: B,
    ) -> Option<EpochExt>
    where
        A: Fn(&H256) -> Option<Header>,
        B: Fn(&H256) -> Option<u64>,
    {
        let last_epoch_length = last_epoch.length();

        if header.number() != (last_epoch.start_number() + last_epoch.length() - 1) {
            return None;
        }

        let last_hash = header.hash();
        let last_difficulty = header.difficulty();
        let target_recip = self.orphan_rate_target_recip();
        let epoch_duration_target = self.epoch_duration_target();

        let last_block_header_in_previous_epoch = if last_epoch.is_genesis() {
            self.genesis_block().header().clone()
        } else {
            get_header(last_epoch.last_block_hash_in_previous_epoch())?
        };

        let start_total_uncles_count =
            total_uncles_count(&last_block_header_in_previous_epoch.hash())
                .expect("block_ext exist");

        let last_total_uncles_count = total_uncles_count(&last_hash).expect("block_ext exist");

        let last_uncles_count = last_total_uncles_count - start_total_uncles_count;

        let epoch_ext = if last_uncles_count > 0 {
            let last_epoch_duration = header
                .timestamp()
                .saturating_sub(last_block_header_in_previous_epoch.timestamp());
            if last_epoch_duration == 0 {
                return None;
            }

            let numerator =
                (last_uncles_count + last_epoch_length) * epoch_duration_target * last_epoch_length;
            let denominator = (target_recip + 1) * last_uncles_count * last_epoch_duration;
            let raw_next_epoch_length = numerator / denominator;
            let next_epoch_length = self.revision_epoch_length(raw_next_epoch_length);

            let raw_difficulty =
                last_difficulty * U256::from(last_uncles_count) * U256::from(target_recip)
                    / U256::from(last_epoch_length);

            let difficulty =
                self.revision_epoch_difficulty(last_difficulty.clone(), raw_difficulty);

            let block_reward = Capacity::shannons(self.epoch_reward().as_u64() / next_epoch_length);
            let remainder_reward =
                Capacity::shannons(self.epoch_reward().as_u64() % next_epoch_length);

            EpochExt::new(
                last_epoch.number() + 1,     // number
                block_reward,
                remainder_reward,            // remainder_reward
                header.hash().to_owned(),    // last_block_hash_in_previous_epoch
                header.number() + 1,         // start
                next_epoch_length,           // length
                difficulty                   // difficulty,
            )
        } else {
            let next_epoch_length = self.max_epoch_length();
            let difficulty = cmp::max(self.min_difficulty().clone(), last_difficulty / 2u64);

            let block_reward = Capacity::shannons(self.epoch_reward().as_u64() / next_epoch_length);
            let remainder_reward =
                Capacity::shannons(self.epoch_reward().as_u64() % next_epoch_length);
            EpochExt::new(
                last_epoch.number() + 1,    // number
                block_reward,
                remainder_reward,           // remainder_reward
                header.hash().to_owned(),   // last_block_hash_in_previous_epoch
                header.number() + 1,        // start
                next_epoch_length,          // length
                difficulty                  // difficulty,
            )
        };

        Some(epoch_ext)
    }
}
