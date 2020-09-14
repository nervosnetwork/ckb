#![allow(clippy::inconsistent_digit_grouping)]

use crate::{
    calculate_block_reward, ChainSpec, OUTPUT_INDEX_DAO,
    OUTPUT_INDEX_SECP256K1_BLAKE160_MULTISIG_ALL, OUTPUT_INDEX_SECP256K1_BLAKE160_SIGHASH_ALL,
};
use ckb_dao_utils::genesis_dao_data_with_satoshi_gift;
use ckb_pow::{Pow, PowEngine};
use ckb_rational::RationalU256;
use ckb_resource::Resource;
use ckb_types::{
    bytes::Bytes,
    constants::{BLOCK_VERSION, TX_VERSION},
    core::{
        BlockBuilder, BlockNumber, BlockView, Capacity, Cycle, EpochExt, EpochNumber,
        EpochNumberWithFraction, HeaderView, Ratio, TransactionBuilder, TransactionView, Version,
    },
    h160, h256,
    packed::{Byte32, CellInput, CellOutput, Script},
    prelude::*,
    u256,
    utilities::{compact_to_difficulty, difficulty_to_compact, DIFF_TWO},
    H160, H256, U256,
};
use std::cmp;
use std::sync::Arc;

// 1.344 billion per year
pub(crate) const DEFAULT_SECONDARY_EPOCH_REWARD: Capacity = Capacity::shannons(613_698_63013698);
// 4.2 billion per year
pub(crate) const INITIAL_PRIMARY_EPOCH_REWARD: Capacity = Capacity::shannons(1_917_808_21917808);
const MAX_UNCLE_NUM: usize = 2;
pub(crate) const TX_PROPOSAL_WINDOW: ProposalWindow = ProposalWindow(2, 10);
// Cellbase outputs are "locked" and require 4 epoch confirmations (approximately 16 hours) before
// they mature sufficiently to be spendable,
// This is to reduce the risk of later txs being reversed if a chain reorganization occurs.
pub(crate) const CELLBASE_MATURITY: EpochNumberWithFraction =
    EpochNumberWithFraction::new_unchecked(4, 0, 1);

const MEDIAN_TIME_BLOCK_COUNT: usize = 37;

// dampening factor
const TAU: u64 = 2;

// We choose 1_000 because it is largest number between MIN_EPOCH_LENGTH and MAX_EPOCH_LENGTH that
// can divide INITIAL_PRIMARY_EPOCH_REWARD and can be divided by ORPHAN_RATE_TARGET_RECIP.
pub(crate) const GENESIS_EPOCH_LENGTH: u64 = 1_000;

// o_ideal = 1/40 = 2.5%
const ORPHAN_RATE_TARGET: RationalU256 = RationalU256::new_raw(U256::one(), u256!("40"));

const MAX_BLOCK_INTERVAL: u64 = 48; // 48s
const MIN_BLOCK_INTERVAL: u64 = 8; // 8s

// cycles of a typical two-in-two-out tx
pub const TWO_IN_TWO_OUT_CYCLES: Cycle = 3_500_000;
// bytes of a typical two-in-two-out tx
pub const TWO_IN_TWO_OUT_BYTES: u64 = 597;
// count of two-in-two-out txs a block should capable to package
const TWO_IN_TWO_OUT_COUNT: u64 = 1_000;
pub(crate) const DEFAULT_EPOCH_DURATION_TARGET: u64 = 4 * 60 * 60; // 4 hours, unit: second
const MILLISECONDS_IN_A_SECOND: u64 = 1000;
const MAX_EPOCH_LENGTH: u64 = DEFAULT_EPOCH_DURATION_TARGET / MIN_BLOCK_INTERVAL; // 1800
const MIN_EPOCH_LENGTH: u64 = DEFAULT_EPOCH_DURATION_TARGET / MAX_BLOCK_INTERVAL; // 300
pub(crate) const DEFAULT_PRIMARY_EPOCH_REWARD_HALVING_INTERVAL: EpochNumber =
    4 * 365 * 24 * 60 * 60 / DEFAULT_EPOCH_DURATION_TARGET; // every 4 years

pub const MAX_BLOCK_BYTES: u64 = TWO_IN_TWO_OUT_BYTES * TWO_IN_TWO_OUT_COUNT;
pub(crate) const MAX_BLOCK_CYCLES: u64 = TWO_IN_TWO_OUT_CYCLES * TWO_IN_TWO_OUT_COUNT;
// 1.5 * TWO_IN_TWO_OUT_COUNT
const MAX_BLOCK_PROPOSALS_LIMIT: u64 = 1_500;
const PROPOSER_REWARD_RATIO: Ratio = Ratio(4, 10);

// Satoshi's pubkey hash in Bitcoin genesis.
pub(crate) const SATOSHI_PUBKEY_HASH: H160 = h160!("0x62e907b15cbf27d5425399ebf6f0fb50ebb88f18");
// Ratio of satoshi cell occupied of capacity,
// only affects genesis cellbase's satoshi lock cells.
pub(crate) const SATOSHI_CELL_OCCUPIED_RATIO: Ratio = Ratio(6, 10);

#[derive(Clone, PartialEq, Debug, Eq, Copy)]
pub struct ProposalWindow(pub BlockNumber, pub BlockNumber);

// "TYPE_ID" in hex
pub const TYPE_ID_CODE_HASH: H256 = h256!("0x545950455f4944");

// 500_000 total difficulty
const MIN_CHAIN_WORK_500K: U256 = u256!("0x3314412053c82802a7");
// const MIN_CHAIN_WORK_1000K: U256 = u256!("0x6f1e2846acc0c9807d");

/// Two protocol parameters w_close and w_far define the closest
/// and farthest on-chain distance between a transaction's proposal
/// and commitment.
///
/// A non-cellbase transaction is committed at height h_c if all of the following conditions are met:
/// 1) it is proposed at height h_p of the same chain, where w_close <= h_c − h_p <= w_far ;
/// 2) it is in the commitment zone of the main chain block with height h_c ;
///
///   ProposalWindow (2, 10)
///       propose
///          \
///           \
///           13 14 [15 16 17 18 19 20 21 22 23]
///                  \_______________________/
///                               \
///                             commit
///

impl ProposalWindow {
    pub fn closest(&self) -> BlockNumber {
        self.0
    }

    pub fn farthest(&self) -> BlockNumber {
        self.1
    }

    pub fn length(&self) -> BlockNumber {
        self.1 - self.0 + 1
    }
}

pub struct ConsensusBuilder {
    inner: Consensus,
}

// Dummy consensus, difficulty can not be zero
impl Default for ConsensusBuilder {
    fn default() -> Self {
        let input = CellInput::new_cellbase_input(0);
        // at least issue some shannons to make dao field valid.
        let output = {
            let empty_output = CellOutput::new_builder().build();
            let occupied = empty_output
                .occupied_capacity(Capacity::zero())
                .expect("default occupied");
            empty_output.as_builder().capacity(occupied.pack()).build()
        };
        let witness = Script::default().into_witness();
        let cellbase = TransactionBuilder::default()
            .input(input)
            .output(output)
            .output_data(Bytes::new().pack())
            .witness(witness)
            .build();

        let epoch_ext = build_genesis_epoch_ext(
            INITIAL_PRIMARY_EPOCH_REWARD,
            DIFF_TWO,
            GENESIS_EPOCH_LENGTH,
            DEFAULT_EPOCH_DURATION_TARGET,
        );
        let primary_issuance =
            calculate_block_reward(INITIAL_PRIMARY_EPOCH_REWARD, GENESIS_EPOCH_LENGTH);
        let secondary_issuance =
            calculate_block_reward(DEFAULT_SECONDARY_EPOCH_REWARD, GENESIS_EPOCH_LENGTH);

        let dao = genesis_dao_data_with_satoshi_gift(
            vec![&cellbase],
            &SATOSHI_PUBKEY_HASH,
            SATOSHI_CELL_OCCUPIED_RATIO,
            primary_issuance,
            secondary_issuance,
        )
        .expect("genesis dao data calculation error!");

        let genesis_block = BlockBuilder::default()
            .compact_target(DIFF_TWO.pack())
            .dao(dao)
            .transaction(cellbase)
            .build();

        ConsensusBuilder::new(genesis_block, epoch_ext)
            .initial_primary_epoch_reward(INITIAL_PRIMARY_EPOCH_REWARD)
    }
}

pub fn build_genesis_epoch_ext(
    epoch_reward: Capacity,
    compact_target: u32,
    genesis_epoch_length: BlockNumber,
    epoch_duration_target: u64,
) -> EpochExt {
    let block_reward = Capacity::shannons(epoch_reward.as_u64() / genesis_epoch_length);
    let remainder_reward = Capacity::shannons(epoch_reward.as_u64() % genesis_epoch_length);

    let genesis_orphan_count = genesis_epoch_length / 40;
    let genesis_hash_rate = compact_to_difficulty(compact_target)
        * (genesis_epoch_length + genesis_orphan_count)
        / epoch_duration_target;

    EpochExt::new_builder()
        .number(0)
        .base_block_reward(block_reward)
        .remainder_reward(remainder_reward)
        .previous_epoch_hash_rate(genesis_hash_rate)
        .last_block_hash_in_previous_epoch(Byte32::zero())
        .start_number(0)
        .length(genesis_epoch_length)
        .compact_target(compact_target)
        .build()
}

pub fn build_genesis_dao_data(
    txs: Vec<&TransactionView>,
    satoshi_pubkey_hash: &H160,
    satoshi_cell_occupied_ratio: Ratio,
    genesis_primary_issuance: Capacity,
    genesis_secondary_issuance: Capacity,
) -> Byte32 {
    genesis_dao_data_with_satoshi_gift(
        txs,
        satoshi_pubkey_hash,
        satoshi_cell_occupied_ratio,
        genesis_primary_issuance,
        genesis_secondary_issuance,
    )
    .expect("genesis dao data calculation error!")
}

impl ConsensusBuilder {
    pub fn new(genesis_block: BlockView, genesis_epoch_ext: EpochExt) -> Self {
        ConsensusBuilder {
            inner: Consensus {
                genesis_hash: genesis_block.header().hash(),
                genesis_block,
                id: "main".to_owned(),
                max_uncles_num: MAX_UNCLE_NUM,
                initial_primary_epoch_reward: INITIAL_PRIMARY_EPOCH_REWARD,
                orphan_rate_target: ORPHAN_RATE_TARGET,
                epoch_duration_target: DEFAULT_EPOCH_DURATION_TARGET,
                secondary_epoch_reward: DEFAULT_SECONDARY_EPOCH_REWARD,
                tx_proposal_window: TX_PROPOSAL_WINDOW,
                pow: Pow::Dummy,
                cellbase_maturity: CELLBASE_MATURITY,
                median_time_block_count: MEDIAN_TIME_BLOCK_COUNT,
                max_block_cycles: MAX_BLOCK_CYCLES,
                max_block_bytes: MAX_BLOCK_BYTES,
                dao_type_hash: None,
                secp256k1_blake160_sighash_all_type_hash: None,
                secp256k1_blake160_multisig_all_type_hash: None,
                genesis_epoch_ext,
                block_version: BLOCK_VERSION,
                tx_version: TX_VERSION,
                type_id_code_hash: TYPE_ID_CODE_HASH,
                proposer_reward_ratio: PROPOSER_REWARD_RATIO,
                max_block_proposals_limit: MAX_BLOCK_PROPOSALS_LIMIT,
                satoshi_pubkey_hash: SATOSHI_PUBKEY_HASH,
                satoshi_cell_occupied_ratio: SATOSHI_CELL_OCCUPIED_RATIO,
                primary_epoch_reward_halving_interval:
                    DEFAULT_PRIMARY_EPOCH_REWARD_HALVING_INTERVAL,
                permanent_difficulty_in_dummy: false,
                min_chain_work: u256!("0x0"),
            },
        }
    }

    fn get_type_hash(&self, output_index: u64) -> Option<Byte32> {
        self.inner
            .genesis_block
            .transaction(0)
            .expect("Genesis must have cellbase")
            .output(output_index as usize)
            .and_then(|output| output.type_().to_opt())
            .map(|type_script| type_script.calc_script_hash())
    }

    pub fn build(mut self) -> Consensus {
        debug_assert!(
            self.inner.genesis_block.difficulty() > U256::zero(),
            "genesis difficulty should greater than zero"
        );
        debug_assert!(
            !self.inner.genesis_block.data().transactions().is_empty()
                && !self
                    .inner
                    .genesis_block
                    .data()
                    .transactions()
                    .get(0)
                    .unwrap()
                    .witnesses()
                    .is_empty(),
            "genesis block must contain the witness for cellbase"
        );

        debug_assert!(
            self.inner.initial_primary_epoch_reward != Capacity::zero(),
            "initial_primary_epoch_reward must be non-zero"
        );

        debug_assert!(
            self.inner.epoch_duration_target() != 0,
            "epoch_duration_target must be non-zero"
        );

        debug_assert!(
            !self.inner.genesis_block.transactions().is_empty()
                && !self.inner.genesis_block.transactions()[0]
                    .witnesses()
                    .is_empty(),
            "genesis block must contain the witness for cellbase"
        );

        let mainnet_genesis =
            ChainSpec::load_from(&Resource::bundled("specs/mainnet.toml".to_string()))
                .expect("load mainnet spec fail")
                .build_genesis()
                .expect("build mainnet genesis fail");
        self.inner.min_chain_work = if self.inner.genesis_block.hash() == mainnet_genesis.hash() {
            MIN_CHAIN_WORK_500K
        } else {
            u256!("0x0")
        };

        self.inner.dao_type_hash = self.get_type_hash(OUTPUT_INDEX_DAO);
        self.inner.secp256k1_blake160_sighash_all_type_hash =
            self.get_type_hash(OUTPUT_INDEX_SECP256K1_BLAKE160_SIGHASH_ALL);
        self.inner.secp256k1_blake160_multisig_all_type_hash =
            self.get_type_hash(OUTPUT_INDEX_SECP256K1_BLAKE160_MULTISIG_ALL);
        self.inner
            .genesis_epoch_ext
            .set_compact_target(self.inner.genesis_block.compact_target());
        self.inner.genesis_hash = self.inner.genesis_block.hash();
        self.inner
    }

    pub fn id(mut self, id: String) -> Self {
        self.inner.id = id;
        self
    }

    pub fn genesis_block(mut self, genesis_block: BlockView) -> Self {
        self.inner.genesis_block = genesis_block;
        self
    }

    #[must_use]
    pub fn initial_primary_epoch_reward(mut self, initial_primary_epoch_reward: Capacity) -> Self {
        self.inner.initial_primary_epoch_reward = initial_primary_epoch_reward;
        self
    }

    #[must_use]
    pub fn secondary_epoch_reward(mut self, secondary_epoch_reward: Capacity) -> Self {
        self.inner.secondary_epoch_reward = secondary_epoch_reward;
        self
    }

    #[must_use]
    pub fn max_block_cycles(mut self, max_block_cycles: Cycle) -> Self {
        self.inner.max_block_cycles = max_block_cycles;
        self
    }

    #[must_use]
    pub fn max_block_bytes(mut self, max_block_bytes: u64) -> Self {
        self.inner.max_block_bytes = max_block_bytes;
        self
    }

    #[must_use]
    pub fn cellbase_maturity(mut self, cellbase_maturity: EpochNumberWithFraction) -> Self {
        self.inner.cellbase_maturity = cellbase_maturity;
        self
    }

    pub fn tx_proposal_window(mut self, proposal_window: ProposalWindow) -> Self {
        self.inner.tx_proposal_window = proposal_window;
        self
    }

    pub fn pow(mut self, pow: Pow) -> Self {
        self.inner.pow = pow;
        self
    }

    pub fn satoshi_pubkey_hash(mut self, pubkey_hash: H160) -> Self {
        self.inner.satoshi_pubkey_hash = pubkey_hash;
        self
    }

    pub fn satoshi_cell_occupied_ratio(mut self, ratio: Ratio) -> Self {
        self.inner.satoshi_cell_occupied_ratio = ratio;
        self
    }

    #[must_use]
    pub fn primary_epoch_reward_halving_interval(mut self, halving_interval: u64) -> Self {
        self.inner.primary_epoch_reward_halving_interval = halving_interval;
        self
    }

    #[must_use]
    pub fn epoch_duration_target(mut self, target: u64) -> Self {
        self.inner.epoch_duration_target = target;
        self
    }

    pub fn permanent_difficulty_in_dummy(mut self, permanent: bool) -> Self {
        self.inner.permanent_difficulty_in_dummy = permanent;
        self
    }
}

#[derive(Clone, Debug)]
pub struct Consensus {
    pub id: String,
    pub genesis_block: BlockView,
    pub genesis_hash: Byte32,
    pub dao_type_hash: Option<Byte32>,
    pub secp256k1_blake160_sighash_all_type_hash: Option<Byte32>,
    pub secp256k1_blake160_multisig_all_type_hash: Option<Byte32>,
    pub initial_primary_epoch_reward: Capacity,
    pub secondary_epoch_reward: Capacity,
    pub max_uncles_num: usize,
    pub orphan_rate_target: RationalU256,
    pub epoch_duration_target: u64,
    pub tx_proposal_window: ProposalWindow,
    pub proposer_reward_ratio: Ratio,
    pub pow: Pow,
    // For each input, if the referenced output transaction is cellbase,
    // it must have at least `cellbase_maturity` confirmations;
    // else reject this transaction.
    pub cellbase_maturity: EpochNumberWithFraction,
    // This parameter indicates the count of past blocks used in the median time calculation
    pub median_time_block_count: usize,
    // Maximum cycles that all the scripts in all the commit transactions can take
    pub max_block_cycles: Cycle,
    // Maximum number of bytes to use for the entire block
    pub max_block_bytes: u64,
    // block version number supported
    pub block_version: Version,
    // tx version number supported
    pub tx_version: Version,
    // "TYPE_ID" in hex
    pub type_id_code_hash: H256,
    // Limit to the number of proposals per block
    pub max_block_proposals_limit: u64,
    pub genesis_epoch_ext: EpochExt,
    // Satoshi's pubkey hash in Bitcoin genesis.
    pub satoshi_pubkey_hash: H160,
    // Ratio of satoshi cell occupied of capacity,
    // only affects genesis cellbase's satoshi lock cells.
    pub satoshi_cell_occupied_ratio: Ratio,
    // Primary reward is cut in half every halving_interval epoch
    // which will occur approximately every 4 years.
    pub primary_epoch_reward_halving_interval: EpochNumber,
    // Keep difficulty be permanent if the pow is dummy
    pub permanent_difficulty_in_dummy: bool,
    // Proof of minimum work during synchronization
    pub min_chain_work: U256,
}

// genesis difficulty should not be zero
impl Default for Consensus {
    fn default() -> Self {
        ConsensusBuilder::default().build()
    }
}

#[allow(clippy::op_ref)]
impl Consensus {
    pub fn genesis_block(&self) -> &BlockView {
        &self.genesis_block
    }

    pub fn proposer_reward_ratio(&self) -> Ratio {
        self.proposer_reward_ratio
    }

    pub fn finalization_delay_length(&self) -> BlockNumber {
        self.tx_proposal_window.farthest() + 1
    }

    pub fn finalize_target(&self, block_number: BlockNumber) -> Option<BlockNumber> {
        if block_number != 0 {
            Some(block_number.saturating_sub(self.finalization_delay_length()))
        } else {
            // Genesis should not reward genesis itself
            None
        }
    }

    pub fn genesis_hash(&self) -> Byte32 {
        self.genesis_hash.clone()
    }

    pub fn dao_type_hash(&self) -> Option<Byte32> {
        self.dao_type_hash.clone()
    }
    pub fn secp256k1_blake160_sighash_all_type_hash(&self) -> Option<Byte32> {
        self.secp256k1_blake160_sighash_all_type_hash.clone()
    }
    pub fn secp256k1_blake160_multisig_all_type_hash(&self) -> Option<Byte32> {
        self.secp256k1_blake160_multisig_all_type_hash.clone()
    }

    pub fn max_uncles_num(&self) -> usize {
        self.max_uncles_num
    }

    pub fn min_difficulty(&self) -> U256 {
        self.genesis_block.difficulty()
    }

    pub fn initial_primary_epoch_reward(&self) -> Capacity {
        self.initial_primary_epoch_reward
    }

    pub fn primary_epoch_reward(&self, epoch_number: u64) -> Capacity {
        let halvings = epoch_number / self.primary_epoch_reward_halving_interval();
        Capacity::shannons(self.initial_primary_epoch_reward.as_u64() >> halvings)
    }

    pub fn primary_epoch_reward_halving_interval(&self) -> EpochNumber {
        self.primary_epoch_reward_halving_interval
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

    pub fn orphan_rate_target(&self) -> &RationalU256 {
        &self.orphan_rate_target
    }

    pub fn pow_engine(&self) -> Arc<dyn PowEngine> {
        self.pow.engine()
    }

    pub fn permanent_difficulty(&self) -> bool {
        self.pow.is_dummy() && self.permanent_difficulty_in_dummy
    }

    pub fn cellbase_maturity(&self) -> EpochNumberWithFraction {
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

    pub fn tx_version(&self) -> Version {
        self.tx_version
    }

    pub fn type_id_code_hash(&self) -> &H256 {
        &self.type_id_code_hash
    }

    pub fn tx_proposal_window(&self) -> ProposalWindow {
        self.tx_proposal_window
    }

    pub fn bounding_hash_rate(
        &self,
        last_epoch_hash_rate: U256,
        last_epoch_previous_hash_rate: U256,
    ) -> U256 {
        if last_epoch_previous_hash_rate == U256::zero() {
            return last_epoch_hash_rate;
        }

        let lower_bound = &last_epoch_previous_hash_rate / TAU;
        if last_epoch_hash_rate < lower_bound {
            return lower_bound;
        }

        let upper_bound = &last_epoch_previous_hash_rate * TAU;
        if last_epoch_hash_rate > upper_bound {
            return upper_bound;
        }
        last_epoch_hash_rate
    }

    pub fn bounding_epoch_length(
        &self,
        length: BlockNumber,
        last_epoch_length: BlockNumber,
    ) -> (BlockNumber, bool) {
        let max_length = cmp::min(self.max_epoch_length(), last_epoch_length * TAU);
        let min_length = cmp::max(self.min_epoch_length(), last_epoch_length / TAU);
        if length > max_length {
            (max_length, true)
        } else if length < min_length {
            (min_length, true)
        } else {
            (length, false)
        }
    }

    pub fn next_epoch_ext<A, B>(
        &self,
        last_epoch: &EpochExt,
        header: &HeaderView,
        get_block_header: A,
        total_uncles_count: B,
    ) -> Option<EpochExt>
    where
        A: Fn(&Byte32) -> Option<HeaderView>,
        B: Fn(&Byte32) -> Option<u64>,
    {
        let last_epoch_length = last_epoch.length();
        let header_number = header.number();
        if header_number != (last_epoch.start_number() + last_epoch_length - 1) {
            return None;
        }

        if self.permanent_difficulty() {
            let dummy_epoch_ext = last_epoch
                .clone()
                .into_builder()
                .number(last_epoch.number() + 1)
                .last_block_hash_in_previous_epoch(header.hash())
                .start_number(header_number + 1)
                .build();
            return Some(dummy_epoch_ext);
        }

        let last_block_header_in_previous_epoch = if last_epoch.is_genesis() {
            self.genesis_block().header()
        } else {
            get_block_header(&last_epoch.last_block_hash_in_previous_epoch())?
        };

        // (1) Computing the Adjusted Hash Rate Estimation
        let last_difficulty = &header.difficulty();
        let last_hash = header.hash();
        let start_total_uncles_count =
            total_uncles_count(&last_block_header_in_previous_epoch.hash())
                .expect("block_ext exist");
        let last_total_uncles_count = total_uncles_count(&last_hash).expect("block_ext exist");
        let last_uncles_count = last_total_uncles_count - start_total_uncles_count;
        let last_epoch_duration = U256::from(cmp::max(
            header
                .timestamp()
                .saturating_sub(last_block_header_in_previous_epoch.timestamp())
                / MILLISECONDS_IN_A_SECOND,
            1,
        ));

        let last_epoch_hash_rate =
            last_difficulty * (last_epoch_length + last_uncles_count) / &last_epoch_duration;

        let adjusted_last_epoch_hash_rate = cmp::max(
            self.bounding_hash_rate(
                last_epoch_hash_rate,
                last_epoch.previous_epoch_hash_rate().to_owned(),
            ),
            U256::one(),
        );

        // (2) Computing the Next Epoch’s Main Chain Block Number
        let orphan_rate_target = self.orphan_rate_target();
        let epoch_duration_target = self.epoch_duration_target();
        let epoch_duration_target_u256 = U256::from(self.epoch_duration_target());
        let last_epoch_length_u256 = U256::from(last_epoch_length);
        let last_orphan_rate = RationalU256::new(
            U256::from(last_uncles_count),
            last_epoch_length_u256.clone(),
        );

        let (next_epoch_length, bound) = if last_uncles_count == 0 {
            (
                cmp::min(self.max_epoch_length(), last_epoch_length * TAU),
                true,
            )
        } else {
            // o_ideal * (1 + o_i ) * L_ideal * C_i,m
            let numerator = orphan_rate_target
                * (&last_orphan_rate + U256::one())
                * &epoch_duration_target_u256
                * &last_epoch_length_u256;
            // o_i * (1 + o_ideal ) * L_i
            let denominator =
                &last_orphan_rate * (orphan_rate_target + U256::one()) * &last_epoch_duration;
            let raw_next_epoch_length = u256_low_u64((numerator / denominator).into_u256());

            self.bounding_epoch_length(raw_next_epoch_length, last_epoch_length)
        };

        // (3) Determining the Next Epoch’s Difficulty
        let next_epoch_length_u256 = U256::from(next_epoch_length);
        let diff_numerator = RationalU256::new(
            &adjusted_last_epoch_hash_rate * epoch_duration_target,
            U256::one(),
        );
        let diff_denominator = if bound {
            if last_orphan_rate.is_zero() {
                RationalU256::new(next_epoch_length_u256, U256::one())
            } else {
                let orphan_rate_estimation_recip = ((&last_orphan_rate + U256::one())
                    * &epoch_duration_target_u256
                    * &last_epoch_length_u256
                    / (&last_orphan_rate * &last_epoch_duration * &next_epoch_length_u256))
                    .saturating_sub_u256(U256::one());

                if orphan_rate_estimation_recip.is_zero() {
                    // small probability event, use o_ideal for now
                    (orphan_rate_target + U256::one()) * next_epoch_length_u256
                } else {
                    let orphan_rate_estimation = RationalU256::one() / orphan_rate_estimation_recip;
                    (orphan_rate_estimation + U256::one()) * next_epoch_length_u256
                }
            }
        } else {
            (orphan_rate_target + U256::one()) * next_epoch_length_u256
        };

        let next_epoch_diff = if diff_numerator > diff_denominator {
            (diff_numerator / diff_denominator).into_u256()
        } else {
            // next_epoch_diff cannot be zero
            U256::one()
        };

        let primary_epoch_reward = self.primary_epoch_reward_of_next_epoch(last_epoch).as_u64();
        let block_reward = Capacity::shannons(primary_epoch_reward / next_epoch_length);
        let remainder_reward = Capacity::shannons(primary_epoch_reward % next_epoch_length);

        let epoch_ext = EpochExt::new_builder()
            .number(last_epoch.number() + 1)
            .base_block_reward(block_reward)
            .remainder_reward(remainder_reward)
            .previous_epoch_hash_rate(adjusted_last_epoch_hash_rate)
            .last_block_hash_in_previous_epoch(header.hash())
            .start_number(header_number + 1)
            .length(next_epoch_length)
            .compact_target(difficulty_to_compact(next_epoch_diff))
            .build();

        Some(epoch_ext)
    }

    pub fn identify_name(&self) -> String {
        let genesis_hash = format!("{:x}", Unpack::<H256>::unpack(&self.genesis_hash));
        format!("/{}/{}", self.id, &genesis_hash[..8])
    }

    pub fn get_secp_type_script_hash(&self) -> Byte32 {
        let secp_cell_data =
            Resource::bundled("specs/cells/secp256k1_blake160_sighash_all".to_string())
                .get()
                .expect("Load secp script data failed");
        let genesis_cellbase = &self.genesis_block().transactions()[0];
        genesis_cellbase
            .outputs()
            .into_iter()
            .zip(genesis_cellbase.outputs_data().into_iter())
            .find(|(_, data)| data.raw_data() == secp_cell_data.as_ref())
            .and_then(|(output, _)| {
                output
                    .type_()
                    .to_opt()
                    .map(|script| script.calc_script_hash())
            })
            .expect("Can not find secp script")
    }

    fn primary_epoch_reward_of_next_epoch(&self, epoch: &EpochExt) -> Capacity {
        if (epoch.number() + 1) % self.primary_epoch_reward_halving_interval() != 0 {
            epoch.primary_reward()
        } else {
            self.primary_epoch_reward(epoch.number() + 1)
        }
    }
}

// most simple and efficient way for now
fn u256_low_u64(u: U256) -> u64 {
    u.0[0]
}

#[cfg(test)]
pub mod test {
    use super::*;
    use ckb_types::core::{capacity_bytes, BlockBuilder, HeaderBuilder, TransactionBuilder};
    use ckb_types::packed::Bytes;

    #[test]
    fn test_init_epoch_reward() {
        let cellbase = TransactionBuilder::default()
            .witness(Bytes::default())
            .build();
        let epoch_ext = build_genesis_epoch_ext(
            capacity_bytes!(100),
            DIFF_TWO,
            GENESIS_EPOCH_LENGTH,
            DEFAULT_EPOCH_DURATION_TARGET,
        );
        let genesis = BlockBuilder::default().transaction(cellbase).build();
        let consensus = ConsensusBuilder::new(genesis, epoch_ext)
            .initial_primary_epoch_reward(capacity_bytes!(100))
            .build();
        assert_eq!(capacity_bytes!(100), consensus.initial_primary_epoch_reward);
    }

    #[test]
    fn test_halving_epoch_reward() {
        let cellbase = TransactionBuilder::default()
            .witness(Bytes::default())
            .build();
        let epoch_ext = build_genesis_epoch_ext(
            capacity_bytes!(100),
            DIFF_TWO,
            GENESIS_EPOCH_LENGTH,
            DEFAULT_EPOCH_DURATION_TARGET,
        );
        let genesis = BlockBuilder::default().transaction(cellbase).build();
        let consensus = ConsensusBuilder::new(genesis.clone(), epoch_ext)
            .initial_primary_epoch_reward(capacity_bytes!(100))
            .build();
        let genesis_epoch = consensus.genesis_epoch_ext();

        let get_block_header = |_hash: &Byte32| Some(genesis.header());
        let total_uncles_count = |_hash: &Byte32| Some(0);
        let header = |number: u64| HeaderBuilder::default().number(number.pack()).build();

        let initial_primary_epoch_reward = genesis_epoch.primary_reward();

        {
            let epoch = consensus
                .next_epoch_ext(
                    &consensus.genesis_epoch_ext(),
                    &header(genesis_epoch.length() - 1),
                    get_block_header,
                    total_uncles_count,
                )
                .expect("test: get next epoch");

            assert_eq!(initial_primary_epoch_reward, epoch.primary_reward());
        }

        let first_halving_epoch_number = consensus.primary_epoch_reward_halving_interval();

        // first_halving_epoch_number - 2
        let epoch = genesis_epoch
            .clone()
            .into_builder()
            .number(first_halving_epoch_number - 2)
            .build();

        // first_halving_epoch_number - 1
        let epoch = consensus
            .next_epoch_ext(
                &epoch,
                &header(epoch.start_number() + epoch.length() - 1),
                get_block_header,
                total_uncles_count,
            )
            .expect("test: get next epoch");
        assert_eq!(initial_primary_epoch_reward, epoch.primary_reward());

        // first_halving_epoch_number
        let epoch = consensus
            .next_epoch_ext(
                &epoch,
                &header(epoch.start_number() + epoch.length() - 1),
                get_block_header,
                total_uncles_count,
            )
            .expect("test: get next epoch");

        assert_eq!(
            initial_primary_epoch_reward.as_u64() / 2,
            epoch.primary_reward().as_u64()
        );

        // first_halving_epoch_number + 1
        let epoch = consensus
            .next_epoch_ext(
                &epoch,
                &header(epoch.start_number() + epoch.length() - 1),
                get_block_header,
                total_uncles_count,
            )
            .expect("test: get next epoch");

        assert_eq!(
            initial_primary_epoch_reward.as_u64() / 2,
            epoch.primary_reward().as_u64()
        );

        // first_halving_epoch_number * 4 - 2
        let epoch = genesis_epoch
            .clone()
            .into_builder()
            .number(first_halving_epoch_number * 4 - 2)
            .base_block_reward(Capacity::shannons(
                initial_primary_epoch_reward.as_u64() / 8 / genesis_epoch.length(),
            ))
            .remainder_reward(Capacity::shannons(
                initial_primary_epoch_reward.as_u64() / 8 % genesis_epoch.length(),
            ))
            .build();

        // first_halving_epoch_number * 4 - 1
        let epoch = consensus
            .next_epoch_ext(
                &epoch,
                &header(epoch.start_number() + epoch.length() - 1),
                get_block_header,
                total_uncles_count,
            )
            .expect("test: get next epoch");

        assert_eq!(
            initial_primary_epoch_reward.as_u64() / 8,
            epoch.primary_reward().as_u64()
        );

        // first_halving_epoch_number * 4
        let epoch = consensus
            .next_epoch_ext(
                &epoch,
                &header(epoch.start_number() + epoch.length() - 1),
                get_block_header,
                total_uncles_count,
            )
            .expect("test: get next epoch");

        assert_eq!(
            initial_primary_epoch_reward.as_u64() / 16,
            epoch.primary_reward().as_u64()
        );
    }
}
