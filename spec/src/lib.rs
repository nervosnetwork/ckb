//! # The Chain Specification
//!
//! By default, when simply running CKB, CKB will connect to the official public Nervos network.
//!
//! In order to run a chain different to the official public one,
//! with a config file specifying `spec = { file = "<the-path-of-spec-file>" }` under `[chain]`.
//!

// Because the limitation of toml library,
// we must put nested config struct in the tail to make it serializable,
// details https://docs.rs/toml/0.5.0/toml/ser/index.html

use crate::consensus::{
    build_genesis_dao_data, build_genesis_epoch_ext, Consensus, ConsensusBuilder,
    SATOSHI_CELL_OCCUPIED_RATIO, SATOSHI_PUBKEY_HASH, TESTNET_ACTIVATION_THRESHOLD,
    TYPE_ID_CODE_HASH,
};
use crate::versionbits::{ActiveMode, Deployment, DeploymentPos};
use ckb_constant::hardfork::{mainnet, testnet};
use ckb_crypto::secp::Privkey;
use ckb_hash::{blake2b_256, new_blake2b};
use ckb_jsonrpc_types::Script;
use ckb_pow::{Pow, PowEngine};
use ckb_resource::{
    Resource, CODE_HASH_DAO, CODE_HASH_SECP256K1_BLAKE160_MULTISIG_ALL,
    CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL, CODE_HASH_SECP256K1_DATA,
};
use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes, hardfork::HardForkSwitch, BlockBuilder, BlockNumber, BlockView, Capacity,
        Cycle, EpochNumber, EpochNumberWithFraction, Ratio, ScriptHashType, TransactionBuilder,
        TransactionView,
    },
    h256, packed,
    prelude::*,
    H160, H256, U128,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

pub use error::SpecError;
pub use hardfork::HardForkConfig;

pub mod consensus;
mod error;
mod hardfork;
pub mod versionbits;

#[cfg(test)]
mod tests;

// Just a random secp256k1 secret key for dep group input cell's lock
const SPECIAL_CELL_PRIVKEY: H256 =
    h256!("0xd0c5c1e2d5af8b6ced3c0800937f996c1fa38c29186cade0cd8b5a73c97aaca3");

/// The output index of SECP256K1/blake160 script in the genesis no.0 transaction
pub const OUTPUT_INDEX_SECP256K1_BLAKE160_SIGHASH_ALL: u64 = 1;
/// The output index of DAO script in the genesis no.0 transaction
pub const OUTPUT_INDEX_DAO: u64 = 2;
/// The output data index of SECP256K1 in the genesis no.0 transaction
pub const OUTPUT_INDEX_SECP256K1_DATA: u64 = 3;
/// The output index of SECP256K1/multisig script in the genesis no.0 transaction
pub const OUTPUT_INDEX_SECP256K1_BLAKE160_MULTISIG_ALL: u64 = 4;

/// The CKB block chain specification
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainSpec {
    /// The spec name, also used identify network
    pub name: String,
    /// The genesis block information
    pub genesis: Genesis,
    /// The block chain parameters
    #[serde(default)]
    pub params: Params,
    /// The block chain pow
    pub pow: Pow,
    #[serde(skip)]
    /// Hash of blake2b_256 spec content bytes, used for check consistency between database and config
    pub hash: packed::Byte32,
}

/// The default_params mod defines the default parameters for CKB Mainnet
pub mod default_params {
    use crate::consensus::{
        CELLBASE_MATURITY, DEFAULT_EPOCH_DURATION_TARGET, DEFAULT_ORPHAN_RATE_TARGET,
        DEFAULT_PRIMARY_EPOCH_REWARD_HALVING_INTERVAL, DEFAULT_SECONDARY_EPOCH_REWARD,
        GENESIS_EPOCH_LENGTH, INITIAL_PRIMARY_EPOCH_REWARD, MAX_BLOCK_BYTES, MAX_BLOCK_CYCLES,
        MAX_BLOCK_PROPOSALS_LIMIT,
    };
    use ckb_types::core::{Capacity, Cycle, EpochNumber};

    /// The default initial_primary_epoch_reward
    ///
    /// Apply to [`initial_primary_epoch_reward`](../consensus/struct.Consensus.html#structfield.initial_primary_epoch_reward)
    pub fn initial_primary_epoch_reward() -> Capacity {
        INITIAL_PRIMARY_EPOCH_REWARD
    }

    /// The default secondary_epoch_reward
    ///
    /// Apply to [`secondary_epoch_reward`](../consensus/struct.Consensus.html#structfield.secondary_epoch_reward)
    pub fn secondary_epoch_reward() -> Capacity {
        DEFAULT_SECONDARY_EPOCH_REWARD
    }

    /// The default max_block_cycles
    ///
    /// Apply to [`max_block_cycles`](../consensus/struct.Consensus.html#structfield.max_block_cycles)
    pub fn max_block_cycles() -> Cycle {
        MAX_BLOCK_CYCLES
    }

    /// The default max_block_bytes
    ///
    /// Apply to [`max_block_bytes`](../consensus/struct.Consensus.html#structfield.max_block_bytes)
    pub fn max_block_bytes() -> u64 {
        MAX_BLOCK_BYTES
    }

    /// The default cellbase_maturity
    ///
    /// Apply to [`cellbase_maturity`](../consensus/struct.Consensus.html#structfield.cellbase_maturity)
    pub fn cellbase_maturity() -> u64 {
        CELLBASE_MATURITY.full_value()
    }

    /// The default primary_epoch_reward_halving_interval
    ///
    /// Apply to [`primary_epoch_reward_halving_interval`](../consensus/struct.Consensus.html#structfield.primary_epoch_reward_halving_interval)
    pub fn primary_epoch_reward_halving_interval() -> EpochNumber {
        DEFAULT_PRIMARY_EPOCH_REWARD_HALVING_INTERVAL
    }

    /// The default epoch_duration
    ///
    /// Apply to [`epoch_duration_target`](../consensus/struct.Consensus.html#structfield.epoch_duration_target)
    pub fn epoch_duration_target() -> u64 {
        DEFAULT_EPOCH_DURATION_TARGET
    }

    /// The default genesis_epoch_length
    ///
    /// Apply to [`genesis_epoch_length`](../consensus/struct.Consensus.html#structfield.genesis_epoch_length)
    pub fn genesis_epoch_length() -> u64 {
        GENESIS_EPOCH_LENGTH
    }

    /// The default max_block_proposals_limit
    ///
    /// Apply to [`max_block_proposals_limit`](../consensus/struct.Consensus.html#structfield.max_block_proposals_limit)
    pub fn max_block_proposals_limit() -> u64 {
        MAX_BLOCK_PROPOSALS_LIMIT
    }

    /// The default permanent_difficulty_in_dummy
    ///
    /// Apply to [`permanent_difficulty_in_dummy`](../consensus/struct.Consensus.html#structfield.permanent_difficulty_in_dummy)
    pub fn permanent_difficulty_in_dummy() -> bool {
        false
    }

    /// The default orphan_rate_target
    ///
    /// Apply to [`orphan_rate_target`](../consensus/struct.Consensus.html#structfield.orphan_rate_target)
    pub fn orphan_rate_target() -> (u32, u32) {
        DEFAULT_ORPHAN_RATE_TARGET
    }
}

/// Parameters for CKB block chain
#[derive(Default, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Params {
    /// The initial_primary_epoch_reward
    ///
    /// See [`initial_primary_epoch_reward`](consensus/struct.Consensus.html#structfield.initial_primary_epoch_reward)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_primary_epoch_reward: Option<Capacity>,
    /// The secondary_epoch_reward
    ///
    /// See [`secondary_epoch_reward`](consensus/struct.Consensus.html#structfield.secondary_epoch_reward)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary_epoch_reward: Option<Capacity>,
    /// The max_block_cycles
    ///
    /// See [`max_block_cycles`](consensus/struct.Consensus.html#structfield.max_block_cycles)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_block_cycles: Option<Cycle>,
    /// The max_block_bytes
    ///
    /// See [`max_block_bytes`](consensus/struct.Consensus.html#structfield.max_block_bytes)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_block_bytes: Option<u64>,
    /// The cellbase_maturity
    ///
    /// See [`cellbase_maturity`](consensus/struct.Consensus.html#structfield.cellbase_maturity)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cellbase_maturity: Option<u64>,
    /// The primary_epoch_reward_halving_interval
    ///
    /// See [`primary_epoch_reward_halving_interval`](consensus/struct.Consensus.html#structfield.primary_epoch_reward_halving_interval)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_epoch_reward_halving_interval: Option<EpochNumber>,
    /// The epoch_duration_target
    ///
    /// See [`epoch_duration_target`](consensus/struct.Consensus.html#structfield.epoch_duration_target)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epoch_duration_target: Option<u64>,
    /// The genesis_epoch_length
    ///
    /// See [`genesis_epoch_length`](consensus/struct.Consensus.html#structfield.genesis_epoch_length)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genesis_epoch_length: Option<BlockNumber>,
    /// The permanent_difficulty_in_dummy
    ///
    /// See [`permanent_difficulty_in_dummy`](consensus/struct.Consensus.html#structfield.permanent_difficulty_in_dummy)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permanent_difficulty_in_dummy: Option<bool>,
    /// The max_block_proposals_limit
    ///
    /// See [`max_block_proposals_limit`](consensus/struct.Consensus.html#structfield.max_block_proposals_limit)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_block_proposals_limit: Option<u64>,
    /// The orphan_rate_target
    ///
    /// See [`orphan_rate_target`](consensus/struct.Consensus.html#structfield.orphan_rate_target)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orphan_rate_target: Option<(u32, u32)>,
    /// The parameters for hard fork features.
    ///
    /// See [`hardfork_switch`](consensus/struct.Consensus.html#structfield.hardfork_switch)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hardfork: Option<HardForkConfig>,
}

impl Params {
    /// Return the `initial_primary_epoch_reward`, otherwise if None, returns the default value
    pub fn initial_primary_epoch_reward(&self) -> Capacity {
        self.initial_primary_epoch_reward
            .unwrap_or_else(default_params::initial_primary_epoch_reward)
    }

    /// Return the `secondary_epoch_reward`, otherwise if None, returns the default value
    pub fn secondary_epoch_reward(&self) -> Capacity {
        self.secondary_epoch_reward
            .unwrap_or_else(default_params::secondary_epoch_reward)
    }

    /// Return the `max_block_cycles`, otherwise if None, returns the default value
    pub fn max_block_cycles(&self) -> Cycle {
        self.max_block_cycles
            .unwrap_or_else(default_params::max_block_cycles)
    }

    /// Return the `max_block_bytes`, otherwise if None, returns the default value
    pub fn max_block_bytes(&self) -> u64 {
        self.max_block_bytes
            .unwrap_or_else(default_params::max_block_bytes)
    }

    /// Return the `cellbase_maturity`, otherwise if None, returns the default value
    pub fn cellbase_maturity(&self) -> u64 {
        self.cellbase_maturity
            .unwrap_or_else(default_params::cellbase_maturity)
    }

    /// Return the `primary_epoch_reward_halving_interval`, otherwise if None, returns the default value
    pub fn primary_epoch_reward_halving_interval(&self) -> EpochNumber {
        self.primary_epoch_reward_halving_interval
            .unwrap_or_else(default_params::primary_epoch_reward_halving_interval)
    }

    /// Return the `permanent_difficulty_in_dummy`, otherwise if None, returns the default value
    pub fn permanent_difficulty_in_dummy(&self) -> bool {
        self.permanent_difficulty_in_dummy
            .unwrap_or_else(default_params::permanent_difficulty_in_dummy)
    }

    /// Return the `epoch_duration_target`, otherwise if None, returns the default value
    pub fn epoch_duration_target(&self) -> u64 {
        self.epoch_duration_target
            .unwrap_or_else(default_params::epoch_duration_target)
    }

    /// Return the `genesis_epoch_length`, otherwise if None, returns the default value
    pub fn genesis_epoch_length(&self) -> BlockNumber {
        self.genesis_epoch_length
            .unwrap_or_else(default_params::genesis_epoch_length)
    }

    /// Return the `max_block_proposals_limit`, otherwise if None, returns the default value
    pub fn max_block_proposals_limit(&self) -> BlockNumber {
        self.max_block_proposals_limit
            .unwrap_or_else(default_params::max_block_proposals_limit)
    }

    /// Return the `orphan_rate_target`, otherwise if None, returns the default value
    pub fn orphan_rate_target(&self) -> (u32, u32) {
        self.orphan_rate_target
            .unwrap_or_else(default_params::orphan_rate_target)
    }
}

/// The genesis information
/// Load from config file.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Genesis {
    /// The genesis block version
    pub version: u32,
    /// The genesis block parent_hash
    pub parent_hash: H256,
    /// The genesis block timestamp
    pub timestamp: u64,
    /// The genesis block compact_target
    pub compact_target: u32,
    /// The genesis block uncles_hash
    pub uncles_hash: H256,
    /// The genesis block hash
    ///
    /// If hash is provided, it will be used to check whether match with actual calculated hash
    pub hash: Option<H256>,
    /// The genesis block nonce
    pub nonce: U128,
    /// The genesis block issued_cells
    ///
    /// Initial token supply
    pub issued_cells: Vec<IssuedCell>,
    /// The genesis cell
    ///
    /// The genesis cell contains a message for identity
    pub genesis_cell: GenesisCell,
    /// The system cells
    ///
    /// The initial system cells, such SECP256K1/blake160, DAO.
    pub system_cells: Vec<SystemCell>,
    /// The system cells' lock
    pub system_cells_lock: Script,
    /// For block 1~11, the reward target is genesis block.
    /// Genesis block must have the lock serialized in the cellbase witness, which is set to `bootstrap_lock`.
    pub bootstrap_lock: Script,
    /// The genesis dep_groups file resource
    ///
    /// see detail [dep-group](https://github.com/nervosnetwork/rfcs/blob/f639fa8b30b5568b895449b7ab3ef4ad40ca077a/rfcs/0022-transaction-structure/0022-transaction-structure.md#dep-group)
    pub dep_groups: Vec<DepGroupResource>,
    /// The burned 25% of Nervos CKBytes in genesis block
    #[serde(default)]
    pub satoshi_gift: SatoshiGift,
}

/// The system cell information
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemCell {
    // NOTE: must put `create_type_id` before `file` otherwise this struct can not serialize
    /// whether crate type script
    pub create_type_id: bool,
    /// Overwrite the cell capacity. Set to None to use the minimal capacity.
    pub capacity: Option<u64>,
    /// The file resource
    pub file: Resource,
}

/// The genesis cell information
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenesisCell {
    /// The genesis cell message
    pub message: String,
    /// The genesis cell lock
    pub lock: Script,
}

/// Initial token supply cell
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssuedCell {
    /// The cell capacity
    pub capacity: Capacity,
    /// The cell lock
    pub lock: Script,
}

/// The genesis dep_group file resources
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DepGroupResource {
    /// The dep_group name
    pub name: String,
    /// The genesis dep_group files
    pub files: Vec<Resource>,
}

/// The burned 25% of Nervos CKBytes in genesis block
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SatoshiGift {
    /// The lock pubkey hash
    pub satoshi_pubkey_hash: H160,
    /// The burned cell virtual occupied, design for NervosDAO
    pub satoshi_cell_occupied_ratio: Ratio,
}

impl Default for SatoshiGift {
    fn default() -> Self {
        SatoshiGift {
            satoshi_pubkey_hash: SATOSHI_PUBKEY_HASH,
            satoshi_cell_occupied_ratio: SATOSHI_CELL_OCCUPIED_RATIO,
        }
    }
}

#[derive(Debug)]
pub(crate) enum SpecLoadError {
    FileNotFound,
    GenesisMismatch { expect: H256, actual: H256 },
}

impl SpecLoadError {
    fn file_not_found() -> Box<Self> {
        Box::new(SpecLoadError::FileNotFound)
    }

    fn genesis_mismatch(expect: H256, actual: H256) -> Box<Self> {
        Box::new(SpecLoadError::GenesisMismatch { expect, actual })
    }
}

impl Error for SpecLoadError {}

impl fmt::Display for SpecLoadError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SpecLoadError::FileNotFound => write!(f, "ChainSpec: file not found"),
            SpecLoadError::GenesisMismatch { expect, actual } => write!(
                f,
                "ChainSpec: genesis hash mismatch, expect {:#x}, actual {:#x}",
                expect, actual
            ),
        }
    }
}

impl ChainSpec {
    /// New ChainSpec instance from load spec file resource
    pub fn load_from(resource: &Resource) -> Result<ChainSpec, Box<dyn Error>> {
        if !resource.exists() {
            return Err(SpecLoadError::file_not_found());
        }
        let config_bytes = resource.get()?;

        let mut spec: ChainSpec = toml::from_slice(&config_bytes)?;
        if let Some(parent) = resource.parent() {
            spec.genesis
                .system_cells
                .iter_mut()
                .for_each(|system_cell| system_cell.file.absolutize(parent));
            spec.genesis
                .dep_groups
                .iter_mut()
                .for_each(|dep_group_resource| {
                    dep_group_resource
                        .files
                        .iter_mut()
                        .for_each(|resource| resource.absolutize(parent))
                });
        }
        // leverage serialize for sanitizing
        spec.hash = packed::Byte32::new(blake2b_256(&toml::to_vec(&spec)?));

        Ok(spec)
    }

    /// The ChainSpec specified pow engine
    pub fn pow_engine(&self) -> Arc<dyn PowEngine> {
        self.pow.engine()
    }

    fn verify_genesis_hash(&self, genesis: &BlockView) -> Result<(), Box<dyn Error>> {
        if let Some(ref expect) = self.genesis.hash {
            let actual: H256 = genesis.hash().unpack();
            if &actual != expect {
                return Err(SpecLoadError::genesis_mismatch(expect.clone(), actual));
            }
        }
        Ok(())
    }

    /// Completes all parameters for hard fork features and creates a hard fork switch.
    ///
    /// Verify the parameters for mainnet and testnet, because all start epoch numbers
    /// for mainnet and testnet are fixed.
    fn build_hardfork_switch(&self) -> Result<HardForkSwitch, Box<dyn Error>> {
        let config = self.params.hardfork.as_ref().cloned().unwrap_or_default();
        match self.name.as_str() {
            mainnet::CHAIN_SPEC_NAME => config.complete_mainnet(),
            testnet::CHAIN_SPEC_NAME => config.complete_testnet(),
            _ => config.complete_with_default(EpochNumber::MAX),
        }
        .map_err(Into::into)
    }

    fn softfork_deployments(&self) -> Option<HashMap<DeploymentPos, Deployment>> {
        match self.name.as_str() {
            mainnet::CHAIN_SPEC_NAME => None,
            testnet::CHAIN_SPEC_NAME => {
                let mut deployments = HashMap::new();
                let light_client = Deployment {
                    bit: 1,
                    start: 5_346, // 2022/11/01
                    timeout: 5_606,
                    min_activation_epoch: 5_676, // 2022/12/25
                    period: 42,
                    active_mode: ActiveMode::Normal,
                    threshold: TESTNET_ACTIVATION_THRESHOLD,
                };
                deployments.insert(DeploymentPos::LightClient, light_client);
                Some(deployments)
            }
            _ => {
                let mut deployments = HashMap::new();
                let light_client = Deployment {
                    bit: 1,
                    start: 0,
                    timeout: 0,
                    min_activation_epoch: 0,
                    period: 10,
                    active_mode: ActiveMode::Always,
                    threshold: TESTNET_ACTIVATION_THRESHOLD,
                };
                deployments.insert(DeploymentPos::LightClient, light_client);
                Some(deployments)
            }
        }
    }

    /// Build consensus instance
    ///
    /// [Consensus](consensus/struct.Consensus.html)
    pub fn build_consensus(&self) -> Result<Consensus, Box<dyn Error>> {
        let hardfork_switch = self.build_hardfork_switch()?;
        let genesis_epoch_ext = build_genesis_epoch_ext(
            self.params.initial_primary_epoch_reward(),
            self.genesis.compact_target,
            self.params.genesis_epoch_length(),
            self.params.epoch_duration_target(),
            self.params.orphan_rate_target(),
        );
        let genesis_block = self.build_genesis()?;
        self.verify_genesis_hash(&genesis_block)?;

        let mut builder = ConsensusBuilder::new(genesis_block, genesis_epoch_ext)
            .id(self.name.clone())
            .cellbase_maturity(EpochNumberWithFraction::from_full_value(
                self.params.cellbase_maturity(),
            ))
            .secondary_epoch_reward(self.params.secondary_epoch_reward())
            .max_block_cycles(self.params.max_block_cycles())
            .max_block_bytes(self.params.max_block_bytes())
            .pow(self.pow.clone())
            .satoshi_pubkey_hash(self.genesis.satoshi_gift.satoshi_pubkey_hash.clone())
            .satoshi_cell_occupied_ratio(self.genesis.satoshi_gift.satoshi_cell_occupied_ratio)
            .primary_epoch_reward_halving_interval(
                self.params.primary_epoch_reward_halving_interval(),
            )
            .initial_primary_epoch_reward(self.params.initial_primary_epoch_reward())
            .epoch_duration_target(self.params.epoch_duration_target())
            .permanent_difficulty_in_dummy(self.params.permanent_difficulty_in_dummy())
            .max_block_proposals_limit(self.params.max_block_proposals_limit())
            .orphan_rate_target(self.params.orphan_rate_target())
            .hardfork_switch(hardfork_switch);

        if let Some(deployments) = self.softfork_deployments() {
            builder = builder.softfork_deployments(deployments);
        }

        Ok(builder.build())
    }

    /// Build genesis block from chain spec
    pub fn build_genesis(&self) -> Result<BlockView, Box<dyn Error>> {
        let special_cell_capacity = {
            let cellbase_transaction_for_special_cell_capacity =
                self.build_cellbase_transaction(capacity_bytes!(500))?;
            // build transaction other than cellbase should return inputs for dao statistics
            let dep_group_transaction_for_special_cell_capacity =
                self.build_dep_group_transaction(&cellbase_transaction_for_special_cell_capacity)?;
            dep_group_transaction_for_special_cell_capacity
                .data()
                .as_reader()
                .raw()
                .outputs()
                .iter()
                .map(|output| Unpack::<Capacity>::unpack(&output.capacity()))
                .try_fold(Capacity::zero(), Capacity::safe_add)
        }?;

        let cellbase_transaction = self.build_cellbase_transaction(special_cell_capacity)?;
        // build transaction other than cellbase should return inputs for dao statistics
        let dep_group_transaction = self.build_dep_group_transaction(&cellbase_transaction)?;

        let genesis_epoch_length = self.params.genesis_epoch_length();
        let genesis_primary_issuance = calculate_block_reward(
            self.params.initial_primary_epoch_reward(),
            genesis_epoch_length,
        );
        let genesis_secondary_issuance =
            calculate_block_reward(self.params.secondary_epoch_reward(), genesis_epoch_length);
        let dao = build_genesis_dao_data(
            vec![&cellbase_transaction, &dep_group_transaction],
            &self.genesis.satoshi_gift.satoshi_pubkey_hash,
            self.genesis.satoshi_gift.satoshi_cell_occupied_ratio,
            genesis_primary_issuance,
            genesis_secondary_issuance,
        );

        let block = BlockBuilder::default()
            .version(self.genesis.version.pack())
            .parent_hash(self.genesis.parent_hash.pack())
            .timestamp(self.genesis.timestamp.pack())
            .compact_target(self.genesis.compact_target.pack())
            .extra_hash(self.genesis.uncles_hash.pack())
            .epoch(EpochNumberWithFraction::new_unchecked(0, 0, 0).pack())
            .dao(dao)
            .nonce(u128::from_le_bytes(self.genesis.nonce.to_le_bytes()).pack())
            .transaction(cellbase_transaction)
            .transaction(dep_group_transaction)
            .build();

        self.check_block(&block)?;
        Ok(block)
    }

    fn check_block(&self, block: &BlockView) -> Result<(), Box<dyn Error>> {
        let mut data_hashes: HashMap<packed::Byte32, (usize, usize)> = HashMap::default();
        let mut type_hashes: HashMap<packed::Byte32, (usize, usize)> = HashMap::default();
        let genesis_cell_lock: packed::Script = self.genesis.genesis_cell.lock.clone().into();
        for (tx_index, tx) in block.transactions().into_iter().enumerate() {
            data_hashes.extend(
                tx.outputs_data()
                    .into_iter()
                    .map(|data| data.raw_data())
                    .enumerate()
                    .filter(|(_, raw_data)| !raw_data.is_empty())
                    .map(|(output_index, raw_data)| {
                        (
                            packed::CellOutput::calc_data_hash(&raw_data),
                            (tx_index, output_index),
                        )
                    }),
            );
            type_hashes.extend(
                tx.outputs()
                    .into_iter()
                    .enumerate()
                    .filter_map(|(output_index, output)| {
                        output
                            .type_()
                            .to_opt()
                            .map(|type_script| (output_index, type_script))
                    })
                    .map(|(output_index, type_script)| {
                        (type_script.calc_script_hash(), (tx_index, output_index))
                    }),
            );
        }
        let all_zero_lock_hash = packed::Byte32::default();
        // Check lock scripts
        for lock_script in block
            .transactions()
            .into_iter()
            .flat_map(|tx| tx.outputs().into_iter().map(move |output| output.lock()))
            .filter(|lock_script| {
                lock_script != &genesis_cell_lock && lock_script.code_hash() != all_zero_lock_hash
            })
        {
            match ScriptHashType::try_from(lock_script.hash_type()).expect("checked data") {
                ScriptHashType::Data => {
                    if !data_hashes.contains_key(&lock_script.code_hash()) {
                        return Err(format!(
                            "Invalid lock script: code_hash={}, hash_type=data",
                            lock_script.code_hash(),
                        )
                        .into());
                    }
                }
                ScriptHashType::Type => {
                    if !type_hashes.contains_key(&lock_script.code_hash()) {
                        return Err(format!(
                            "Invalid lock script: code_hash={}, hash_type=type",
                            lock_script.code_hash(),
                        )
                        .into());
                    }
                }
                ScriptHashType::Data1 => {
                    if !data_hashes.contains_key(&lock_script.code_hash()) {
                        return Err(format!(
                            "Invalid lock script: code_hash={}, hash_type=data1",
                            lock_script.code_hash(),
                        )
                        .into());
                    }
                }
            }
        }

        // Check system cells data hash
        let check_cells_data_hash = |tx_index, output_index, hash: &H256| {
            if data_hashes.get(&hash.pack()) != Some(&(tx_index, output_index)) {
                return Err(format!(
                    "Invalid output data for tx-index: {}, output-index: {}, expected data hash: {:x}",
                    tx_index, output_index,
                    hash,
                ));
            }
            Ok(())
        };
        check_cells_data_hash(
            0,
            OUTPUT_INDEX_SECP256K1_BLAKE160_SIGHASH_ALL as usize,
            &CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL,
        )?;
        check_cells_data_hash(0, OUTPUT_INDEX_DAO as usize, &CODE_HASH_DAO)?;
        check_cells_data_hash(
            0,
            OUTPUT_INDEX_SECP256K1_DATA as usize,
            &CODE_HASH_SECP256K1_DATA,
        )?;
        check_cells_data_hash(
            0,
            OUTPUT_INDEX_SECP256K1_BLAKE160_MULTISIG_ALL as usize,
            &CODE_HASH_SECP256K1_BLAKE160_MULTISIG_ALL,
        )?;

        Ok(())
    }

    fn build_cellbase_transaction(
        &self,
        special_cell_capacity: Capacity,
    ) -> Result<TransactionView, Box<dyn Error>> {
        let input = packed::CellInput::new_cellbase_input(0);
        let mut outputs = Vec::<packed::CellOutput>::with_capacity(
            1 + self.genesis.system_cells.len() + self.genesis.issued_cells.len(),
        );
        let mut outputs_data = Vec::with_capacity(outputs.capacity());

        // Layout of genesis cellbase:
        // - genesis cell, which contains a message and can never be spent.
        // - system cells, which stores the built-in code blocks.
        // - special issued cell, for dep group cell in next transaction
        // - issued cells
        let (output, data) = self.genesis.genesis_cell.build_output()?;
        outputs.push(output);
        outputs_data.push(data);

        // The first output cell is genesis cell
        let system_cells_output_index_start = 1;
        let (system_cells_outputs, system_cells_data): (Vec<_>, Vec<_>) = self
            .genesis
            .system_cells
            .iter()
            .enumerate()
            .map(|(index, system_cell)| {
                system_cell.build_output(
                    &input,
                    system_cells_output_index_start + index as u64,
                    &self.genesis.system_cells_lock,
                )
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .unzip();
        outputs.extend(system_cells_outputs);
        outputs_data.extend(system_cells_data);

        let special_issued_lock = packed::Script::new_builder()
            .args(secp_lock_arg(&Privkey::from(SPECIAL_CELL_PRIVKEY.clone())).pack())
            .code_hash(CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL.clone().pack())
            .hash_type(ScriptHashType::Data.into())
            .build();
        let special_issued_cell = packed::CellOutput::new_builder()
            .capacity(special_cell_capacity.pack())
            .lock(special_issued_lock)
            .build();
        outputs.push(special_issued_cell);
        outputs_data.push(Bytes::new());

        outputs.extend(
            self.genesis
                .issued_cells
                .iter()
                .map(IssuedCell::build_output),
        );
        outputs_data.extend(self.genesis.issued_cells.iter().map(|_| Bytes::new()));

        let script: packed::Script = self.genesis.bootstrap_lock.clone().into();

        let tx = TransactionBuilder::default()
            .input(input)
            .outputs(outputs)
            .witness(script.into_witness())
            .outputs_data(
                outputs_data
                    .iter()
                    .map(|d| d.pack())
                    .collect::<Vec<packed::Bytes>>(),
            )
            .build();
        Ok(tx)
    }

    fn build_dep_group_transaction(
        &self,
        cellbase_tx: &TransactionView,
    ) -> Result<TransactionView, Box<dyn Error>> {
        fn find_out_point_by_data_hash(
            tx: &TransactionView,
            data_hash: &packed::Byte32,
        ) -> Option<packed::OutPoint> {
            tx.outputs_data()
                .into_iter()
                .position(|data| {
                    let hash = packed::CellOutput::calc_data_hash(&data.raw_data());
                    &hash == data_hash
                })
                .map(|index| packed::OutPoint::new(tx.hash(), index as u32))
        }

        let (outputs, outputs_data): (Vec<_>, Vec<_>) = self
            .genesis
            .dep_groups
            .iter()
            .map(|dep_group| {
                let out_points: Vec<_> = dep_group
                    .files
                    .iter()
                    .map(|res| {
                        let data: Bytes = res.get()?.into_owned().into();
                        let data_hash = packed::CellOutput::calc_data_hash(&data);
                        let out_point = find_out_point_by_data_hash(cellbase_tx, &data_hash)
                            .ok_or_else(|| {
                                format!("Can not find {} in genesis cellbase transaction", res)
                            })?;
                        Ok(out_point)
                    })
                    .collect::<Result<_, Box<dyn Error>>>()?;

                let data = out_points.pack().as_bytes();
                let cell = packed::CellOutput::new_builder()
                    .lock(self.genesis.system_cells_lock.clone().into())
                    .build_exact_capacity(Capacity::bytes(data.len())?)?;
                Ok((cell, data.pack()))
            })
            .collect::<Result<Vec<(packed::CellOutput, packed::Bytes)>, Box<dyn Error>>>()?
            .into_iter()
            .unzip();

        let privkey = Privkey::from(SPECIAL_CELL_PRIVKEY.clone());
        let lock_arg = secp_lock_arg(&privkey);
        let input_out_point = cellbase_tx
            .outputs()
            .into_iter()
            .position(|output| Unpack::<Bytes>::unpack(&output.lock().args()) == lock_arg)
            .map(|index| packed::OutPoint::new(cellbase_tx.hash(), index as u32))
            .expect("Get special issued input failed");
        let input = packed::CellInput::new(input_out_point, 0);

        let secp_data_out_point =
            find_out_point_by_data_hash(cellbase_tx, &CODE_HASH_SECP256K1_DATA.pack())
                .ok_or_else(|| String::from("Get secp data out point failed"))?;
        let secp_blake160_out_point = find_out_point_by_data_hash(
            cellbase_tx,
            &CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL.pack(),
        )
        .ok_or_else(|| String::from("Get secp blake160 out point failed"))?;
        let cell_deps = vec![
            packed::CellDep::new_builder()
                .out_point(secp_data_out_point)
                .build(),
            packed::CellDep::new_builder()
                .out_point(secp_blake160_out_point)
                .build(),
        ];
        let tx = TransactionBuilder::default()
            .cell_deps(cell_deps.clone())
            .input(input.clone())
            .outputs(outputs.clone())
            .outputs_data(outputs_data.clone())
            .build();

        let tx_hash: H256 = tx.hash().unpack();
        let message = H256::from(blake2b_256(&tx_hash));
        let sig = privkey.sign_recoverable(&message).expect("sign");
        let witness = Bytes::from(sig.serialize()).pack();

        Ok(TransactionBuilder::default()
            .cell_deps(cell_deps)
            .input(input)
            .outputs(outputs)
            .outputs_data(outputs_data)
            .witness(witness)
            .build())
    }
}

impl GenesisCell {
    fn build_output(&self) -> Result<(packed::CellOutput, Bytes), Box<dyn Error>> {
        let data: Bytes = self.message.as_bytes().to_owned().into();
        let cell = packed::CellOutput::new_builder()
            .lock(self.lock.clone().into())
            .build_exact_capacity(Capacity::bytes(data.len())?)?;
        Ok((cell, data))
    }
}

impl IssuedCell {
    fn build_output(&self) -> packed::CellOutput {
        packed::CellOutput::new_builder()
            .lock(self.lock.clone().into())
            .capacity(self.capacity.pack())
            .build()
    }
}

impl SystemCell {
    fn build_output(
        &self,
        input: &packed::CellInput,
        output_index: u64,
        lock: &Script,
    ) -> Result<(packed::CellOutput, Bytes), Box<dyn Error>> {
        let data: Bytes = self.file.get()?.into_owned().into();
        let type_script = if self.create_type_id {
            Some(build_type_id_script(input, output_index))
        } else {
            None
        };
        let builder = packed::CellOutput::new_builder()
            .type_(type_script.pack())
            .lock(lock.clone().into());

        let data_len = Capacity::bytes(data.len())?;
        let cell = if let Some(capacity) = self.capacity {
            let cell = builder.capacity(capacity.pack()).build();
            let occupied_capacity = cell.occupied_capacity(data_len)?.as_u64();
            if occupied_capacity > capacity {
                return Err(format!(
                    "Insufficient capacity to create system cell at index {}, \
                     occupied / capacity = {} / {}",
                    output_index, occupied_capacity, capacity
                )
                .into());
            }
            cell
        } else {
            builder.build_exact_capacity(data_len)?
        };

        Ok((cell, data))
    }
}

fn secp_lock_arg(privkey: &Privkey) -> Bytes {
    let pubkey_data = privkey.pubkey().expect("Get pubkey failed").serialize();
    Bytes::from((&blake2b_256(&pubkey_data)[0..20]).to_owned())
}

/// Shortcut for build genesis type_id script from specified output_index
pub fn build_genesis_type_id_script(output_index: u64) -> packed::Script {
    build_type_id_script(&packed::CellInput::new_cellbase_input(0), output_index)
}

pub(crate) fn build_type_id_script(input: &packed::CellInput, output_index: u64) -> packed::Script {
    let mut blake2b = new_blake2b();
    blake2b.update(input.as_slice());
    blake2b.update(&output_index.to_le_bytes());
    let mut ret = [0; 32];
    blake2b.finalize(&mut ret);
    let script_arg = Bytes::from(ret.to_vec());
    packed::Script::new_builder()
        .code_hash(TYPE_ID_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(script_arg.pack())
        .build()
}

/// Shortcut for calculate block_reward helper method
pub fn calculate_block_reward(epoch_reward: Capacity, epoch_length: BlockNumber) -> Capacity {
    let epoch_reward = epoch_reward.as_u64();
    Capacity::shannons({
        if epoch_reward % epoch_length != 0 {
            epoch_reward / epoch_length + 1
        } else {
            epoch_reward / epoch_length
        }
    })
}
