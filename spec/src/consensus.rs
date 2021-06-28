//! Consensus defines various tweakable parameters of a given instance of the CKB system.
//!

#![allow(clippy::inconsistent_digit_grouping)]

use crate::{
    calculate_block_reward, OUTPUT_INDEX_DAO, OUTPUT_INDEX_SECP256K1_BLAKE160_MULTISIG_ALL,
    OUTPUT_INDEX_SECP256K1_BLAKE160_SIGHASH_ALL,
};
use ckb_dao_utils::genesis_dao_data_with_satoshi_gift;
use ckb_pow::{Pow, PowEngine};
use ckb_rational::RationalU256;
use ckb_resource::Resource;
use ckb_traits::{BlockEpoch, EpochProvider};
use ckb_types::{
    bytes::Bytes,
    constants::{BLOCK_VERSION, TX_VERSION},
    core::{
        hardfork::HardForkSwitch, BlockBuilder, BlockNumber, BlockView, Capacity, Cycle, EpochExt,
        EpochNumber, EpochNumberWithFraction, HeaderView, Ratio, TransactionBuilder,
        TransactionView, Version,
    },
    h160, h256,
    packed::{Byte32, CellInput, CellOutput, Script},
    prelude::*,
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
pub(crate) const DEFAULT_ORPHAN_RATE_TARGET: (u32, u32) = (1, 40);

const MAX_BLOCK_INTERVAL: u64 = 48; // 48s
const MIN_BLOCK_INTERVAL: u64 = 8; // 8s

/// cycles of a typical two-in-two-out tx.
pub const TWO_IN_TWO_OUT_CYCLES: Cycle = 3_500_000;
/// bytes of a typical two-in-two-out tx.
pub const TWO_IN_TWO_OUT_BYTES: u64 = 597;
/// count of two-in-two-out txs a block should capable to package.
const TWO_IN_TWO_OUT_COUNT: u64 = 1_000;
pub(crate) const DEFAULT_EPOCH_DURATION_TARGET: u64 = 4 * 60 * 60; // 4 hours, unit: second
const MILLISECONDS_IN_A_SECOND: u64 = 1000;
const MAX_EPOCH_LENGTH: u64 = DEFAULT_EPOCH_DURATION_TARGET / MIN_BLOCK_INTERVAL; // 1800
const MIN_EPOCH_LENGTH: u64 = DEFAULT_EPOCH_DURATION_TARGET / MAX_BLOCK_INTERVAL; // 300
pub(crate) const DEFAULT_PRIMARY_EPOCH_REWARD_HALVING_INTERVAL: EpochNumber =
    4 * 365 * 24 * 60 * 60 / DEFAULT_EPOCH_DURATION_TARGET; // every 4 years

/// The default maximum allowed size in bytes for a block
pub const MAX_BLOCK_BYTES: u64 = TWO_IN_TWO_OUT_BYTES * TWO_IN_TWO_OUT_COUNT;
pub(crate) const MAX_BLOCK_CYCLES: u64 = TWO_IN_TWO_OUT_CYCLES * TWO_IN_TWO_OUT_COUNT;

/// The default maximum allowed amount of proposals for a block
///
/// Default value from 1.5 * TWO_IN_TWO_OUT_COUNT
pub const MAX_BLOCK_PROPOSALS_LIMIT: u64 = 1_500;
const PROPOSER_REWARD_RATIO: Ratio = Ratio::new(4, 10);

// Satoshi's pubkey hash in Bitcoin genesis.
pub(crate) const SATOSHI_PUBKEY_HASH: H160 = h160!("0x62e907b15cbf27d5425399ebf6f0fb50ebb88f18");
// Ratio of satoshi cell occupied of capacity,
// only affects genesis cellbase's satoshi lock cells.
pub(crate) const SATOSHI_CELL_OCCUPIED_RATIO: Ratio = Ratio::new(6, 10);

/// The struct represent CKB two-step-transaction-confirmation params
///
/// [two-step-transaction-confirmation params](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0020-ckb-consensus-protocol/0020-ckb-consensus-protocol.md#two-step-transaction-confirmation)
#[derive(Clone, PartialEq, Debug, Eq, Copy)]
pub struct ProposalWindow(pub BlockNumber, pub BlockNumber);

/// "TYPE_ID" in hex
pub const TYPE_ID_CODE_HASH: H256 = h256!("0x545950455f4944");

/// Two protocol parameters w_close and w_far define the closest
/// and farthest on-chain distance between a transaction's proposal
/// and commitment.
///
/// A non-cellbase transaction is committed at height h_c if all of the following conditions are met:
/// 1) it is proposed at height h_p of the same chain, where w_close <= h_c − h_p <= w_far ;
/// 2) it is in the commitment zone of the main chain block with height h_c ;
///
/// ```text
/// ProposalWindow (2, 10)
///     propose
///        \
///         \
///         13 14 [15 16 17 18 19 20 21 22 23]
///                \_______________________/
///                             \
///                           commit
/// ```
///
impl ProposalWindow {
    /// The w_close parameter
    pub fn closest(&self) -> BlockNumber {
        self.0
    }

    /// The w_far parameter
    pub fn farthest(&self) -> BlockNumber {
        self.1
    }

    /// The proposal window length
    pub fn length(&self) -> BlockNumber {
        self.1 - self.0 + 1
    }
}

/// The Consensus factory, which can be used in order to configure the properties of a new Consensus.
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
            DEFAULT_ORPHAN_RATE_TARGET,
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
            .epoch(EpochNumberWithFraction::new_unchecked(0, 0, 0).pack())
            .dao(dao)
            .transaction(cellbase)
            .build();

        ConsensusBuilder::new(genesis_block, epoch_ext)
            .initial_primary_epoch_reward(INITIAL_PRIMARY_EPOCH_REWARD)
    }
}

/// Build the epoch information of genesis block
pub fn build_genesis_epoch_ext(
    epoch_reward: Capacity,
    compact_target: u32,
    genesis_epoch_length: BlockNumber,
    epoch_duration_target: u64,
    genesis_orphan_rate: (u32, u32),
) -> EpochExt {
    let block_reward = Capacity::shannons(epoch_reward.as_u64() / genesis_epoch_length);
    let remainder_reward = Capacity::shannons(epoch_reward.as_u64() % genesis_epoch_length);

    let genesis_orphan_count =
        genesis_epoch_length * genesis_orphan_rate.0 as u64 / genesis_orphan_rate.1 as u64;
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

/// Build the dao data of genesis block
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
    /// Generates the base configuration for build a Consensus, from which configuration methods can be chained.
    pub fn new(genesis_block: BlockView, genesis_epoch_ext: EpochExt) -> Self {
        let orphan_rate_target = RationalU256::new_raw(
            U256::from(DEFAULT_ORPHAN_RATE_TARGET.0),
            U256::from(DEFAULT_ORPHAN_RATE_TARGET.1),
        );
        ConsensusBuilder {
            inner: Consensus {
                genesis_hash: genesis_block.header().hash(),
                genesis_block,
                id: "main".to_owned(),
                max_uncles_num: MAX_UNCLE_NUM,
                initial_primary_epoch_reward: INITIAL_PRIMARY_EPOCH_REWARD,
                orphan_rate_target,
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
                hardfork_switch: HardForkSwitch::new_without_any_enabled(),
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

    /// Build a new Consensus by taking ownership of the `Builder`, and returns a [`Consensus`].
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

    /// Names the network.
    pub fn id(mut self, id: String) -> Self {
        self.inner.id = id;
        self
    }

    /// Sets genesis_block for the new Consensus.
    pub fn genesis_block(mut self, genesis_block: BlockView) -> Self {
        self.inner.genesis_block = genesis_block;
        self
    }

    /// Sets initial_primary_epoch_reward for the new Consensus.
    #[must_use]
    pub fn initial_primary_epoch_reward(mut self, initial_primary_epoch_reward: Capacity) -> Self {
        self.inner.initial_primary_epoch_reward = initial_primary_epoch_reward;
        self
    }

    /// Sets orphan_rate_target for the new Consensus.
    pub fn orphan_rate_target(mut self, orphan_rate_target: (u32, u32)) -> Self {
        self.inner.orphan_rate_target = RationalU256::new_raw(
            U256::from(orphan_rate_target.0),
            U256::from(orphan_rate_target.1),
        );
        self
    }

    /// Sets secondary_epoch_reward for the new Consensus.
    #[must_use]
    pub fn secondary_epoch_reward(mut self, secondary_epoch_reward: Capacity) -> Self {
        self.inner.secondary_epoch_reward = secondary_epoch_reward;
        self
    }

    /// Sets max_block_cycles for the new Consensus.
    #[must_use]
    pub fn max_block_cycles(mut self, max_block_cycles: Cycle) -> Self {
        self.inner.max_block_cycles = max_block_cycles;
        self
    }

    /// Sets max_block_bytes for the new Consensus.
    #[must_use]
    pub fn max_block_bytes(mut self, max_block_bytes: u64) -> Self {
        self.inner.max_block_bytes = max_block_bytes;
        self
    }

    /// Sets cellbase_maturity for the new Consensus.
    #[must_use]
    pub fn cellbase_maturity(mut self, cellbase_maturity: EpochNumberWithFraction) -> Self {
        self.inner.cellbase_maturity = cellbase_maturity;
        self
    }

    /// Sets median_time_block_count for the new Consensus.
    pub fn median_time_block_count(mut self, median_time_block_count: usize) -> Self {
        self.inner.median_time_block_count = median_time_block_count;
        self
    }

    /// Sets tx_proposal_window for the new Consensus.
    pub fn tx_proposal_window(mut self, proposal_window: ProposalWindow) -> Self {
        self.inner.tx_proposal_window = proposal_window;
        self
    }

    /// Sets pow for the new Consensus.
    pub fn pow(mut self, pow: Pow) -> Self {
        self.inner.pow = pow;
        self
    }

    /// Sets satoshi_pubkey_hash for the new Consensus.
    pub fn satoshi_pubkey_hash(mut self, pubkey_hash: H160) -> Self {
        self.inner.satoshi_pubkey_hash = pubkey_hash;
        self
    }

    /// Sets satoshi_cell_occupied_ratio for the new Consensus.
    pub fn satoshi_cell_occupied_ratio(mut self, ratio: Ratio) -> Self {
        self.inner.satoshi_cell_occupied_ratio = ratio;
        self
    }

    /// Sets primary_epoch_reward_halving_interval for the new Consensus.
    #[must_use]
    pub fn primary_epoch_reward_halving_interval(mut self, halving_interval: u64) -> Self {
        self.inner.primary_epoch_reward_halving_interval = halving_interval;
        self
    }

    /// Sets expected epoch_duration_target for the new Consensus.
    #[must_use]
    pub fn epoch_duration_target(mut self, target: u64) -> Self {
        self.inner.epoch_duration_target = target;
        self
    }

    /// Sets permanent_difficulty_in_dummy for the new Consensus.
    ///
    /// [dynamic-difficulty-adjustment-mechanism](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0020-ckb-consensus-protocol/0020-ckb-consensus-protocol.md#dynamic-difficulty-adjustment-mechanism)
    /// may be a disturbance in dev chain, set permanent_difficulty_in_dummy to true will disable dynamic difficulty adjustment mechanism. keep difficulty unchanged.
    /// Work only under dummy Pow
    #[must_use]
    pub fn permanent_difficulty_in_dummy(mut self, permanent: bool) -> Self {
        self.inner.permanent_difficulty_in_dummy = permanent;
        self
    }

    /// Sets max_block_proposals_limit for the new Consensus.
    #[must_use]
    pub fn max_block_proposals_limit(mut self, max_block_proposals_limit: u64) -> Self {
        self.inner.max_block_proposals_limit = max_block_proposals_limit;
        self
    }

    /// Sets a hard fork switch for the new Consensus.
    pub fn hardfork_switch(mut self, hardfork_switch: HardForkSwitch) -> Self {
        self.inner.hardfork_switch = hardfork_switch;
        self
    }
}

/// Struct Consensus defines various parameters that influence chain consensus
#[derive(Clone, Debug)]
pub struct Consensus {
    /// Names the network.
    pub id: String,
    /// The genesis block
    pub genesis_block: BlockView,
    /// The genesis block hash
    pub genesis_hash: Byte32,
    /// The dao type hash
    ///
    /// [nervos-dao](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0024-ckb-system-script-list/0024-ckb-system-script-list.md#nervos-dao)
    pub dao_type_hash: Option<Byte32>,
    /// The secp256k1_blake160_sighash_all_type_hash
    ///
    /// [SECP256K1/blake160](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0024-ckb-system-script-list/0024-ckb-system-script-list.md#secp256k1blake160)
    pub secp256k1_blake160_sighash_all_type_hash: Option<Byte32>,
    /// The secp256k1_blake160_multisig_all_type_hash
    ///
    /// [SECP256K1/multisig](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0024-ckb-system-script-list/0024-ckb-system-script-list.md#secp256k1multisig)
    pub secp256k1_blake160_multisig_all_type_hash: Option<Byte32>,
    /// The initial primary_epoch_reward
    ///
    /// [token-issuance](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0015-ckb-cryptoeconomics/0015-ckb-cryptoeconomics.md#token-issuance)
    pub initial_primary_epoch_reward: Capacity,
    /// The secondary primary_epoch_reward
    ///
    /// [token-issuance](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0015-ckb-cryptoeconomics/0015-ckb-cryptoeconomics.md#token-issuance)
    pub secondary_epoch_reward: Capacity,
    /// The maximum amount of uncles allowed for a block
    pub max_uncles_num: usize,
    /// The expected orphan_rate
    pub orphan_rate_target: RationalU256,
    /// The expected epoch_duration
    pub epoch_duration_target: u64,
    /// The two-step-transaction-confirmation proposal window
    pub tx_proposal_window: ProposalWindow,
    /// The two-step-transaction-confirmation proposer reward ratio
    pub proposer_reward_ratio: Ratio,
    /// The pow parameters
    pub pow: Pow,
    /// The Cellbase maturity
    /// For each input, if the referenced output transaction is cellbase,
    /// it must have at least `cellbase_maturity` confirmations;
    /// else reject this transaction.
    pub cellbase_maturity: EpochNumberWithFraction,
    /// This parameter indicates the count of past blocks used in the median time calculation
    pub median_time_block_count: usize,
    /// Maximum cycles that all the scripts in all the commit transactions can take
    pub max_block_cycles: Cycle,
    /// Maximum number of bytes to use for the entire block
    pub max_block_bytes: u64,
    /// The block version number supported
    pub block_version: Version,
    /// The tx version number supported
    pub tx_version: Version,
    /// The "TYPE_ID" in hex
    pub type_id_code_hash: H256,
    /// The Limit to the number of proposals per block
    pub max_block_proposals_limit: u64,
    /// The genesis epoch information
    pub genesis_epoch_ext: EpochExt,
    /// Satoshi's pubkey hash in Bitcoin genesis.
    pub satoshi_pubkey_hash: H160,
    /// Ratio of satoshi cell occupied of capacity,
    /// only affects genesis cellbase's satoshi lock cells.
    pub satoshi_cell_occupied_ratio: Ratio,
    /// Primary reward is cut in half every halving_interval epoch
    /// which will occur approximately every 4 years.
    pub primary_epoch_reward_halving_interval: EpochNumber,
    /// Keep difficulty be permanent if the pow is dummy
    pub permanent_difficulty_in_dummy: bool,
    /// A switch to select hard fork features base on the epoch number.
    pub hardfork_switch: HardForkSwitch,
}

// genesis difficulty should not be zero
impl Default for Consensus {
    fn default() -> Self {
        ConsensusBuilder::default().build()
    }
}

#[allow(clippy::op_ref)]
impl Consensus {
    /// The genesis block
    pub fn genesis_block(&self) -> &BlockView {
        &self.genesis_block
    }

    /// The two-step-transaction-confirmation proposer reward ratio
    pub fn proposer_reward_ratio(&self) -> Ratio {
        self.proposer_reward_ratio
    }

    /// The two-step-transaction-confirmation block reward delay length
    pub fn finalization_delay_length(&self) -> BlockNumber {
        self.tx_proposal_window.farthest() + 1
    }

    /// Get block reward finalize number from specified block number
    pub fn finalize_target(&self, block_number: BlockNumber) -> Option<BlockNumber> {
        if block_number != 0 {
            Some(block_number.saturating_sub(self.finalization_delay_length()))
        } else {
            // Genesis should not reward genesis itself
            None
        }
    }

    /// The genesis block hash
    pub fn genesis_hash(&self) -> Byte32 {
        self.genesis_hash.clone()
    }

    /// The dao type hash
    ///
    /// [nervos-dao](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0024-ckb-system-script-list/0024-ckb-system-script-list.md#nervos-dao)
    pub fn dao_type_hash(&self) -> Option<Byte32> {
        self.dao_type_hash.clone()
    }

    /// The secp256k1_blake160_sighash_all_type_hash
    ///
    /// [SECP256K1/blake160](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0024-ckb-system-script-list/0024-ckb-system-script-list.md#secp256k1blake160)
    pub fn secp256k1_blake160_sighash_all_type_hash(&self) -> Option<Byte32> {
        self.secp256k1_blake160_sighash_all_type_hash.clone()
    }

    /// The secp256k1_blake160_multisig_all_type_hash
    ///
    /// [SECP256K1/multisig](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0024-ckb-system-script-list/0024-ckb-system-script-list.md#secp256k1multisig)
    pub fn secp256k1_blake160_multisig_all_type_hash(&self) -> Option<Byte32> {
        self.secp256k1_blake160_multisig_all_type_hash.clone()
    }

    /// The maximum amount of uncles allowed for a block
    pub fn max_uncles_num(&self) -> usize {
        self.max_uncles_num
    }

    /// The minimum difficulty (genesis_block difficulty)
    pub fn min_difficulty(&self) -> U256 {
        self.genesis_block.difficulty()
    }

    /// The minimum difficulty (genesis_block difficulty)
    pub fn initial_primary_epoch_reward(&self) -> Capacity {
        self.initial_primary_epoch_reward
    }

    /// The initial primary_epoch_reward
    ///
    /// [token-issuance](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0015-ckb-cryptoeconomics/0015-ckb-cryptoeconomics.md#token-issuance)
    pub fn primary_epoch_reward(&self, epoch_number: u64) -> Capacity {
        let halvings = epoch_number / self.primary_epoch_reward_halving_interval();
        Capacity::shannons(self.initial_primary_epoch_reward.as_u64() >> halvings)
    }

    /// Primary reward is cut in half every halving_interval epoch
    /// which will occur approximately every 4 years.
    pub fn primary_epoch_reward_halving_interval(&self) -> EpochNumber {
        self.primary_epoch_reward_halving_interval
    }

    /// The expected epoch_duration
    pub fn epoch_duration_target(&self) -> u64 {
        self.epoch_duration_target
    }

    /// The genesis epoch information
    pub fn genesis_epoch_ext(&self) -> &EpochExt {
        &self.genesis_epoch_ext
    }

    /// The maximum epoch length
    pub fn max_epoch_length(&self) -> BlockNumber {
        MAX_EPOCH_LENGTH
    }

    /// The minimum epoch length
    pub fn min_epoch_length(&self) -> BlockNumber {
        MIN_EPOCH_LENGTH
    }

    /// The secondary primary_epoch_reward
    ///
    /// [token-issuance](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0015-ckb-cryptoeconomics/0015-ckb-cryptoeconomics.md#token-issuance)
    pub fn secondary_epoch_reward(&self) -> Capacity {
        self.secondary_epoch_reward
    }

    /// The expected orphan_rate
    pub fn orphan_rate_target(&self) -> &RationalU256 {
        &self.orphan_rate_target
    }

    /// The pow_engine
    pub fn pow_engine(&self) -> Arc<dyn PowEngine> {
        self.pow.engine()
    }

    /// The permanent_difficulty mode
    pub fn permanent_difficulty(&self) -> bool {
        self.pow.is_dummy() && self.permanent_difficulty_in_dummy
    }

    /// The cellbase_maturity
    pub fn cellbase_maturity(&self) -> EpochNumberWithFraction {
        self.cellbase_maturity
    }

    /// This parameter indicates the count of past blocks used in the median time calculation
    pub fn median_time_block_count(&self) -> usize {
        self.median_time_block_count
    }

    /// Maximum cycles that all the scripts in all the commit transactions can take
    pub fn max_block_cycles(&self) -> Cycle {
        self.max_block_cycles
    }

    /// Maximum number of bytes to use for the entire block
    pub fn max_block_bytes(&self) -> u64 {
        self.max_block_bytes
    }

    /// The Limit to the number of proposals per block
    pub fn max_block_proposals_limit(&self) -> u64 {
        self.max_block_proposals_limit
    }

    /// The current block version
    pub fn block_version(&self) -> Version {
        self.block_version
    }

    /// The current transaction version
    pub fn tx_version(&self) -> Version {
        self.tx_version
    }

    /// The "TYPE_ID" in hex
    pub fn type_id_code_hash(&self) -> &H256 {
        &self.type_id_code_hash
    }

    /// The two-step-transaction-confirmation proposal window
    pub fn tx_proposal_window(&self) -> ProposalWindow {
        self.tx_proposal_window
    }

    // Apply the dampening filter on hash_rate estimation calculate
    fn bounding_hash_rate(
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

    // Apply the dampening filter on epoch_length calculate
    fn bounding_epoch_length(
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

    /// The [dynamic-difficulty-adjustment-mechanism](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0020-ckb-consensus-protocol/0020-ckb-consensus-protocol.md#dynamic-difficulty-adjustment-mechanism)
    /// implementation
    pub fn next_epoch_ext<P: EpochProvider>(
        &self,
        header: &HeaderView,
        provider: &P,
    ) -> Option<NextBlockEpoch> {
        provider
            .get_block_epoch(header)
            .map(|block_epoch| match block_epoch {
                BlockEpoch::NonTailBlock { epoch } => NextBlockEpoch::NonHeadBlock(epoch),
                BlockEpoch::TailBlock {
                    epoch,
                    epoch_uncles_count,
                    epoch_duration_in_milliseconds,
                } => {
                    if self.permanent_difficulty() {
                        let dummy_epoch_ext = epoch
                            .clone()
                            .into_builder()
                            .number(epoch.number() + 1)
                            .last_block_hash_in_previous_epoch(header.hash())
                            .start_number(header.number() + 1)
                            .build();
                        NextBlockEpoch::HeadBlock(dummy_epoch_ext)
                    } else {
                        // (1) Computing the Adjusted Hash Rate Estimation
                        let last_difficulty = &header.difficulty();
                        let last_epoch_duration = U256::from(cmp::max(
                            epoch_duration_in_milliseconds / MILLISECONDS_IN_A_SECOND,
                            1,
                        ));

                        let last_epoch_hash_rate = last_difficulty
                            * (epoch.length() + epoch_uncles_count)
                            / &last_epoch_duration;

                        let adjusted_last_epoch_hash_rate = cmp::max(
                            self.bounding_hash_rate(
                                last_epoch_hash_rate,
                                epoch.previous_epoch_hash_rate().to_owned(),
                            ),
                            U256::one(),
                        );

                        // (2) Computing the Next Epoch’s Main Chain Block Number
                        let orphan_rate_target = self.orphan_rate_target();
                        let epoch_duration_target = self.epoch_duration_target();
                        let epoch_duration_target_u256 = U256::from(self.epoch_duration_target());
                        let last_epoch_length_u256 = U256::from(epoch.length());
                        let last_orphan_rate = RationalU256::new(
                            U256::from(epoch_uncles_count),
                            last_epoch_length_u256.clone(),
                        );

                        let (next_epoch_length, bound) = if epoch_uncles_count == 0 {
                            (
                                cmp::min(self.max_epoch_length(), epoch.length() * TAU),
                                true,
                            )
                        } else {
                            // o_ideal * (1 + o_i ) * L_ideal * C_i,m
                            let numerator = orphan_rate_target
                                * (&last_orphan_rate + U256::one())
                                * &epoch_duration_target_u256
                                * &last_epoch_length_u256;
                            // o_i * (1 + o_ideal ) * L_i
                            let denominator = &last_orphan_rate
                                * (orphan_rate_target + U256::one())
                                * &last_epoch_duration;
                            let raw_next_epoch_length =
                                u256_low_u64((numerator / denominator).into_u256());

                            self.bounding_epoch_length(raw_next_epoch_length, epoch.length())
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
                                let orphan_rate_estimation_recip = ((&last_orphan_rate
                                    + U256::one())
                                    * &epoch_duration_target_u256
                                    * &last_epoch_length_u256
                                    / (&last_orphan_rate
                                        * &last_epoch_duration
                                        * &next_epoch_length_u256))
                                    .saturating_sub_u256(U256::one());

                                if orphan_rate_estimation_recip.is_zero() {
                                    // small probability event, use o_ideal for now
                                    (orphan_rate_target + U256::one()) * next_epoch_length_u256
                                } else {
                                    let orphan_rate_estimation =
                                        RationalU256::one() / orphan_rate_estimation_recip;
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

                        let primary_epoch_reward =
                            self.primary_epoch_reward_of_next_epoch(&epoch).as_u64();
                        let block_reward =
                            Capacity::shannons(primary_epoch_reward / next_epoch_length);
                        let remainder_reward =
                            Capacity::shannons(primary_epoch_reward % next_epoch_length);

                        let epoch_ext = EpochExt::new_builder()
                            .number(epoch.number() + 1)
                            .base_block_reward(block_reward)
                            .remainder_reward(remainder_reward)
                            .previous_epoch_hash_rate(adjusted_last_epoch_hash_rate)
                            .last_block_hash_in_previous_epoch(header.hash())
                            .start_number(header.number() + 1)
                            .length(next_epoch_length)
                            .compact_target(difficulty_to_compact(next_epoch_diff))
                            .build();

                        NextBlockEpoch::HeadBlock(epoch_ext)
                    }
                }
            })
    }

    /// The network identify name, used for network identify protocol
    pub fn identify_name(&self) -> String {
        let genesis_hash = format!("{:x}", Unpack::<H256>::unpack(&self.genesis_hash));
        format!("/{}/{}", self.id, &genesis_hash[..8])
    }

    /// The secp256k1_blake160_sighash_all code hash
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

    /// Returns the hardfork switch.
    pub fn hardfork_switch(&self) -> &HardForkSwitch {
        &self.hardfork_switch
    }
}

/// Trait for consensus provider.
pub trait ConsensusProvider {
    /// Returns the `Consensus`.
    fn get_consensus(&self) -> &Consensus;
}

/// Corresponding epoch information of next block
pub enum NextBlockEpoch {
    /// Next block is the head block of epoch
    HeadBlock(EpochExt),
    /// Next block is not the head block of epoch
    NonHeadBlock(EpochExt),
}

impl NextBlockEpoch {
    /// Returns epoch information
    pub fn epoch(self) -> EpochExt {
        match self {
            Self::HeadBlock(epoch_ext) => epoch_ext,
            Self::NonHeadBlock(epoch_ext) => epoch_ext,
        }
    }

    /// Is a head block of epoch
    pub fn is_head(&self) -> bool {
        matches!(*self, Self::HeadBlock(_))
    }
}

impl From<Consensus> for ckb_jsonrpc_types::Consensus {
    fn from(consensus: Consensus) -> Self {
        Self {
            id: consensus.id,
            genesis_hash: consensus.genesis_hash.unpack(),
            dao_type_hash: consensus.dao_type_hash.map(|h| h.unpack()),
            secp256k1_blake160_sighash_all_type_hash: consensus
                .secp256k1_blake160_sighash_all_type_hash
                .map(|h| h.unpack()),
            secp256k1_blake160_multisig_all_type_hash: consensus
                .secp256k1_blake160_multisig_all_type_hash
                .map(|h| h.unpack()),
            initial_primary_epoch_reward: consensus.initial_primary_epoch_reward.into(),
            secondary_epoch_reward: consensus.secondary_epoch_reward.into(),
            max_uncles_num: (consensus.max_uncles_num as u64).into(),
            orphan_rate_target: consensus.orphan_rate_target,
            epoch_duration_target: consensus.epoch_duration_target.into(),
            tx_proposal_window: ckb_jsonrpc_types::ProposalWindow {
                closest: consensus.tx_proposal_window.0.into(),
                farthest: consensus.tx_proposal_window.1.into(),
            },
            proposer_reward_ratio: RationalU256::new_raw(
                consensus.proposer_reward_ratio.numer().into(),
                consensus.proposer_reward_ratio.denom().into(),
            ),
            cellbase_maturity: consensus.cellbase_maturity.into(),
            median_time_block_count: (consensus.median_time_block_count as u64).into(),
            max_block_cycles: consensus.max_block_cycles.into(),
            max_block_bytes: consensus.max_block_bytes.into(),
            block_version: consensus.block_version.into(),
            tx_version: consensus.tx_version.into(),
            type_id_code_hash: consensus.type_id_code_hash,
            max_block_proposals_limit: consensus.max_block_proposals_limit.into(),
            primary_epoch_reward_halving_interval: consensus
                .primary_epoch_reward_halving_interval
                .into(),
            permanent_difficulty_in_dummy: consensus.permanent_difficulty_in_dummy,
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
    use ckb_traits::{BlockEpoch, EpochProvider};
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
            DEFAULT_ORPHAN_RATE_TARGET,
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
            DEFAULT_ORPHAN_RATE_TARGET,
        );
        let genesis = BlockBuilder::default().transaction(cellbase).build();
        let consensus = ConsensusBuilder::new(genesis, epoch_ext)
            .initial_primary_epoch_reward(capacity_bytes!(100))
            .build();
        let genesis_epoch = consensus.genesis_epoch_ext();

        let header = |number: u64| HeaderBuilder::default().number(number.pack()).build();

        struct DummyEpochProvider(EpochExt);
        impl EpochProvider for DummyEpochProvider {
            fn get_epoch_ext(&self, _block_header: &HeaderView) -> Option<EpochExt> {
                Some(self.0.clone())
            }
            fn get_block_epoch(&self, block_header: &HeaderView) -> Option<BlockEpoch> {
                let block_epoch =
                    if block_header.number() == self.0.start_number() + self.0.length() - 1 {
                        BlockEpoch::TailBlock {
                            epoch: self.0.clone(),
                            epoch_uncles_count: 0,
                            epoch_duration_in_milliseconds: DEFAULT_EPOCH_DURATION_TARGET * 1000,
                        }
                    } else {
                        BlockEpoch::NonTailBlock {
                            epoch: self.0.clone(),
                        }
                    };
                Some(block_epoch)
            }
        }
        let initial_primary_epoch_reward = genesis_epoch.primary_reward();

        {
            let epoch = consensus
                .next_epoch_ext(
                    &header(genesis_epoch.length() - 1),
                    &DummyEpochProvider(genesis_epoch.clone()),
                )
                .expect("test: get next epoch")
                .epoch();

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
                &header(epoch.start_number() + epoch.length() - 1),
                &DummyEpochProvider(epoch),
            )
            .expect("test: get next epoch")
            .epoch();
        assert_eq!(initial_primary_epoch_reward, epoch.primary_reward());

        // first_halving_epoch_number
        let epoch = consensus
            .next_epoch_ext(
                &header(epoch.start_number() + epoch.length() - 1),
                &DummyEpochProvider(epoch),
            )
            .expect("test: get next epoch")
            .epoch();

        assert_eq!(
            initial_primary_epoch_reward.as_u64() / 2,
            epoch.primary_reward().as_u64()
        );

        // first_halving_epoch_number + 1
        let epoch = consensus
            .next_epoch_ext(
                &header(epoch.start_number() + epoch.length() - 1),
                &DummyEpochProvider(epoch),
            )
            .expect("test: get next epoch")
            .epoch();

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
                &header(epoch.start_number() + epoch.length() - 1),
                &DummyEpochProvider(epoch),
            )
            .expect("test: get next epoch")
            .epoch();

        assert_eq!(
            initial_primary_epoch_reward.as_u64() / 8,
            epoch.primary_reward().as_u64()
        );

        // first_halving_epoch_number * 4
        let epoch = consensus
            .next_epoch_ext(
                &header(epoch.start_number() + epoch.length() - 1),
                &DummyEpochProvider(epoch),
            )
            .expect("test: get next epoch")
            .epoch();

        assert_eq!(
            initial_primary_epoch_reward.as_u64() / 16,
            epoch.primary_reward().as_u64()
        );
    }
}
