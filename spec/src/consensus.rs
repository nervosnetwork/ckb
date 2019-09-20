use crate::{
    OUTPUT_INDEX_DAO, OUTPUT_INDEX_SECP256K1_BLAKE160_SIGHASH_ALL,
    OUTPUT_INDEX_SECP256K1_RIPEMD160_SHA256_SIGHASH_ALL,
};
use ckb_dao_utils::genesis_dao_data_with_satoshi_gift;
use ckb_pow::{Pow, PowEngine};
use ckb_rational::RationalU256;
use ckb_resource::Resource;
use ckb_types::{
    constants::BLOCK_VERSION,
    core::{
        capacity_bytes, BlockBuilder, BlockNumber, BlockView, Capacity, Cycle, EpochExt,
        HeaderView, Ratio, TransactionBuilder, Version,
    },
    h160,
    packed::{Byte32, CellInput, Script},
    prelude::*,
    u256, H160, H256, U256,
};
use std::cmp;
use std::sync::Arc;

// TODO: add secondary reward for miner
pub(crate) const DEFAULT_SECONDARY_EPOCH_REWARD: Capacity = capacity_bytes!(600_000);
pub(crate) const DEFAULT_EPOCH_REWARD: Capacity = capacity_bytes!(1_250_000);
const MAX_UNCLE_NUM: usize = 2;
const TX_PROPOSAL_WINDOW: ProposalWindow = ProposalWindow(2, 10);
// Cellbase outputs are "locked" and require 4 * MAX_EPOCH_LENGTH(1800) confirmations(approximately 16 hours)
// before they mature sufficiently to be spendable,
// This is to reduce the risk of later txs being reversed if a chain reorganization occurs.
pub(crate) const CELLBASE_MATURITY: BlockNumber = 4 * MAX_EPOCH_LENGTH;
// TODO: should adjust this value based on CKB average block time
const MEDIAN_TIME_BLOCK_COUNT: usize = 37;

// dampening factor
const TAU: u64 = 2;

// o_ideal = 1/40 = 2.5%
const ORPHAN_RATE_TARGET: RationalU256 = RationalU256::new_raw(U256::one(), u256!("40"));
const GENESIS_ORPHAN_COUNT: u64 = GENESIS_EPOCH_LENGTH / 40;

const MAX_BLOCK_INTERVAL: u64 = 30; // 30s
const MIN_BLOCK_INTERVAL: u64 = 8; // 8s

// cycles of a typical two-in-two-out tx
const TWO_IN_TWO_OUT_CYCLES: Cycle = 13_335_200;
// bytes of a typical two-in-two-out tx
const TWO_IN_TWO_OUT_BYTES: u64 = 589;
// count of two-in-two-out txs a block should capable to package
// approximately equal to 50_000_000_000 / TWO_IN_TWO_OUT_CYCLES
const TWO_IN_TWO_OUT_COUNT: u64 = 3875;
const EPOCH_DURATION_TARGET: u64 = 4 * 60 * 60; // 4 hours, unit: second
const MILLISECONDS_IN_A_SECOND: u64 = 1000;
const MAX_EPOCH_LENGTH: u64 = EPOCH_DURATION_TARGET / MIN_BLOCK_INTERVAL; // 1800
const MIN_EPOCH_LENGTH: u64 = EPOCH_DURATION_TARGET / MAX_BLOCK_INTERVAL; // 480

// We choose 1_000 because it is largest number between MIN_EPOCH_LENGTH and MAX_EPOCH_LENGTH that
// can divide DEFAULT_EPOCH_REWARD and can be divided by ORPHAN_RATE_TARGET_RECIP.
const GENESIS_EPOCH_LENGTH: u64 = 1_000;

const MAX_BLOCK_BYTES: u64 = TWO_IN_TWO_OUT_BYTES * TWO_IN_TWO_OUT_COUNT;
pub(crate) const MAX_BLOCK_CYCLES: u64 = TWO_IN_TWO_OUT_CYCLES * TWO_IN_TWO_OUT_COUNT;
const MAX_BLOCK_PROPOSALS_LIMIT: u64 = 3_000;
const PROPOSER_REWARD_RATIO: Ratio = Ratio(4, 10);

// Satoshi's pubkey hash in Bitcoin genesis.
pub(crate) const SATOSHI_PUBKEY_HASH: H160 = h160!("0x62e907b15cbf27d5425399ebf6f0fb50ebb88f18");
// Ratio of satoshi cell occupied of capacity,
// only affects genesis cellbase's satoshi lock cells.
pub(crate) const SATOSHI_CELL_OCCUPIED_RATIO: Ratio = Ratio(6, 10);

#[derive(Clone, PartialEq, Debug, Eq, Copy)]
pub struct ProposalWindow(pub BlockNumber, pub BlockNumber);

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
    id: String,
    genesis_block: BlockView,
    genesis_hash: Byte32,
    epoch_reward: Capacity,
    secondary_epoch_reward: Capacity,
    max_uncles_num: usize,
    orphan_rate_target: RationalU256,
    epoch_duration_target: u64,
    tx_proposal_window: ProposalWindow,
    proposer_reward_ratio: Ratio,
    pow: Pow,
    cellbase_maturity: BlockNumber,
    median_time_block_count: usize,
    max_block_cycles: Cycle,
    max_block_bytes: u64,
    block_version: Version,
    max_block_proposals_limit: u64,
    genesis_epoch_ext: EpochExt,
    satoshi_pubkey_hash: H160,
    satoshi_cell_occupied_ratio: Ratio,
}

// genesis difficulty should not be zero
impl Default for ConsensusBuilder {
    fn default() -> Self {
        let input = CellInput::new_cellbase_input(0);
        let witness = Script::default().into_witness();
        let cellbase = TransactionBuilder::default()
            .input(input)
            .witness(witness)
            .build();
        let dao = genesis_dao_data_with_satoshi_gift(
            vec![&cellbase],
            &SATOSHI_PUBKEY_HASH,
            SATOSHI_CELL_OCCUPIED_RATIO,
        )
        .unwrap();
        let genesis_block = BlockBuilder::default()
            .difficulty(U256::one().pack())
            .dao(dao)
            .transaction(cellbase)
            .build();

        ConsensusBuilder::new(genesis_block, DEFAULT_EPOCH_REWARD)
    }
}

impl ConsensusBuilder {
    pub fn new(genesis_block: BlockView, epoch_reward: Capacity) -> Self {
        debug_assert!(
            genesis_block.difficulty() > U256::zero(),
            "genesis difficulty should greater than zero"
        );

        debug_assert!(
            !genesis_block.transactions().is_empty()
                && !genesis_block.transactions()[0].witnesses().is_empty(),
            "genesis block must contain the witness for cellbase"
        );

        let genesis_header = genesis_block.header();
        let block_reward = Capacity::shannons(epoch_reward.as_u64() / GENESIS_EPOCH_LENGTH);
        let remainder_reward = Capacity::shannons(epoch_reward.as_u64() % GENESIS_EPOCH_LENGTH);

        let genesis_hash_rate = genesis_block.header().difficulty()
            * (GENESIS_EPOCH_LENGTH + GENESIS_ORPHAN_COUNT)
            / EPOCH_DURATION_TARGET;

        let genesis_epoch_ext = EpochExt::new_builder()
            .number(0)
            .base_block_reward(block_reward)
            .remainder_reward(remainder_reward)
            .previous_epoch_hash_rate(genesis_hash_rate)
            .last_block_hash_in_previous_epoch(Byte32::zero())
            .start_number(0)
            .length(GENESIS_EPOCH_LENGTH)
            .difficulty(genesis_header.difficulty())
            .build();

        ConsensusBuilder {
            genesis_hash: genesis_header.hash(),
            genesis_block,
            id: "main".to_owned(),
            max_uncles_num: MAX_UNCLE_NUM,
            epoch_reward,
            orphan_rate_target: ORPHAN_RATE_TARGET,
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
            proposer_reward_ratio: PROPOSER_REWARD_RATIO,
            max_block_proposals_limit: MAX_BLOCK_PROPOSALS_LIMIT,
            satoshi_pubkey_hash: SATOSHI_PUBKEY_HASH,
            satoshi_cell_occupied_ratio: SATOSHI_CELL_OCCUPIED_RATIO,
        }
    }

    pub fn build(self) -> Consensus {
        let ConsensusBuilder {
            genesis_hash,
            genesis_block,
            id,
            max_uncles_num,
            epoch_reward,
            orphan_rate_target,
            epoch_duration_target,
            secondary_epoch_reward,
            tx_proposal_window,
            pow,
            cellbase_maturity,
            median_time_block_count,
            max_block_cycles,
            max_block_bytes,
            genesis_epoch_ext,
            block_version,
            proposer_reward_ratio,
            max_block_proposals_limit,
            satoshi_pubkey_hash,
            satoshi_cell_occupied_ratio,
        } = self;

        let get_type_hash = |output_index: u64| {
            genesis_block
                .transaction(0)
                .expect("Genesis must have cellbase")
                .output(output_index as usize)
                .and_then(|output| output.type_().to_opt())
                .map(|type_script| type_script.calc_script_hash())
        };
        let dao_type_hash = get_type_hash(OUTPUT_INDEX_DAO);
        let secp_blake160_type_hash = get_type_hash(OUTPUT_INDEX_SECP256K1_BLAKE160_SIGHASH_ALL);
        let secp_ripemd160_type_hash =
            get_type_hash(OUTPUT_INDEX_SECP256K1_RIPEMD160_SHA256_SIGHASH_ALL);

        Consensus {
            genesis_hash,
            dao_type_hash,
            secp_blake160_type_hash,
            secp_ripemd160_type_hash,
            genesis_block,
            id,
            max_uncles_num,
            epoch_reward,
            orphan_rate_target,
            epoch_duration_target,
            secondary_epoch_reward,
            tx_proposal_window,
            pow,
            cellbase_maturity,
            median_time_block_count,
            max_block_cycles,
            max_block_bytes,
            genesis_epoch_ext,
            block_version,
            proposer_reward_ratio,
            max_block_proposals_limit,
            satoshi_pubkey_hash,
            satoshi_cell_occupied_ratio,
        }
    }

    pub fn id(mut self, id: String) -> Self {
        self.id = id;
        self
    }

    pub fn genesis_block(mut self, genesis_block: BlockView) -> Self {
        debug_assert!(
            !genesis_block.data().transactions().is_empty()
                && !genesis_block
                    .data()
                    .transactions()
                    .get(0)
                    .unwrap()
                    .witnesses()
                    .is_empty(),
            "genesis block must contain the witness for cellbase"
        );
        self.genesis_epoch_ext
            .set_difficulty(genesis_block.difficulty());
        self.genesis_hash = genesis_block.hash();
        self.genesis_block = genesis_block;
        self
    }

    pub fn genesis_epoch_ext(mut self, genesis_epoch_ext: EpochExt) -> Self {
        self.genesis_epoch_ext = genesis_epoch_ext;
        self
    }

    pub fn epoch_reward(mut self, epoch_reward: Capacity) -> Self {
        self.epoch_reward = epoch_reward;
        self
    }

    #[must_use]
    pub fn secondary_epoch_reward(mut self, secondary_epoch_reward: Capacity) -> Self {
        self.secondary_epoch_reward = secondary_epoch_reward;
        self
    }

    #[must_use]
    pub fn max_block_cycles(mut self, max_block_cycles: Cycle) -> Self {
        self.max_block_cycles = max_block_cycles;
        self
    }

    #[must_use]
    pub fn cellbase_maturity(mut self, cellbase_maturity: BlockNumber) -> Self {
        self.cellbase_maturity = cellbase_maturity;
        self
    }

    pub fn tx_proposal_window(mut self, proposal_window: ProposalWindow) -> Self {
        self.tx_proposal_window = proposal_window;
        self
    }

    pub fn pow(mut self, pow: Pow) -> Self {
        self.pow = pow;
        self
    }

    pub fn satoshi_pubkey_hash(mut self, pubkey_hash: H160) -> Self {
        self.satoshi_pubkey_hash = pubkey_hash;
        self
    }

    pub fn satoshi_cell_occupied_ratio(mut self, ratio: Ratio) -> Self {
        self.satoshi_cell_occupied_ratio = ratio;
        self
    }
}

#[derive(Clone, Debug)]
pub struct Consensus {
    pub id: String,
    pub genesis_block: BlockView,
    pub genesis_hash: Byte32,
    pub dao_type_hash: Option<Byte32>,
    pub secp_blake160_type_hash: Option<Byte32>,
    pub secp_ripemd160_type_hash: Option<Byte32>,
    pub epoch_reward: Capacity,
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
    // Satoshi's pubkey hash in Bitcoin genesis.
    pub satoshi_pubkey_hash: H160,
    // Ratio of satoshi cell occupied of capacity,
    // only affects genesis cellbase's satoshi lock cells.
    pub satoshi_cell_occupied_ratio: Ratio,
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
    pub fn secp_blake160_type_hash(&self) -> Option<Byte32> {
        self.secp_blake160_type_hash.clone()
    }
    pub fn secp_ripemd160_type_hash(&self) -> Option<Byte32> {
        self.secp_ripemd160_type_hash.clone()
    }

    pub fn max_uncles_num(&self) -> usize {
        self.max_uncles_num
    }

    pub fn min_difficulty(&self) -> U256 {
        self.genesis_block.difficulty()
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

    pub fn orphan_rate_target(&self) -> &RationalU256 {
        &self.orphan_rate_target
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
        let next_epoch_diff = if bound {
            if last_orphan_rate.is_zero() {
                let denominator = U256::from(next_epoch_length);
                (diff_numerator / denominator).into_u256()
            } else {
                let orphan_rate_estimation_recip = ((&last_orphan_rate + U256::one())
                    * &epoch_duration_target_u256
                    * &last_epoch_length_u256
                    / (&last_orphan_rate * &last_epoch_duration * &next_epoch_length_u256))
                    .saturating_sub_u256(U256::one());

                let denominator = if orphan_rate_estimation_recip.is_zero() {
                    // small probability event, use o_ideal for now
                    (orphan_rate_target + U256::one()) * next_epoch_length_u256
                } else {
                    let orphan_rate_estimation = RationalU256::one() / orphan_rate_estimation_recip;
                    (orphan_rate_estimation + U256::one()) * next_epoch_length_u256
                };
                (diff_numerator / denominator).into_u256()
            }
        } else {
            let denominator = (orphan_rate_target + U256::one()) * next_epoch_length_u256;
            (diff_numerator / denominator).into_u256()
        };

        debug_assert!(
            next_epoch_diff > U256::zero(),
            "next_epoch_diff should greater than one"
        );

        let block_reward = Capacity::shannons(self.epoch_reward().as_u64() / next_epoch_length);
        let remainder_reward = Capacity::shannons(self.epoch_reward().as_u64() % next_epoch_length);

        let epoch_ext = EpochExt::new_builder()
            .number(last_epoch.number() + 1)
            .base_block_reward(block_reward)
            .remainder_reward(remainder_reward)
            .previous_epoch_hash_rate(adjusted_last_epoch_hash_rate)
            .last_block_hash_in_previous_epoch(header.hash())
            .start_number(header_number + 1)
            .length(next_epoch_length)
            .difficulty(next_epoch_diff)
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
}

// most simple and efficient way for now
fn u256_low_u64(u: U256) -> u64 {
    u.0[0]
}

#[cfg(test)]
pub mod test {
    use super::*;
    use ckb_types::core::{BlockBuilder, TransactionBuilder};

    #[test]
    fn test_init_epoch_reward() {
        let cellbase = TransactionBuilder::default().witness(vec![].pack()).build();
        let genesis = BlockBuilder::default().transaction(cellbase).build();
        let consensus = ConsensusBuilder::new(genesis, capacity_bytes!(100)).build();
        assert_eq!(capacity_bytes!(100), consensus.epoch_reward);
    }
}
