//! # The Chain Specification
//!
//! By default, when simply running CKB, CKB will connect to the official public Nervos network.
//!
//! In order to run a chain different to the official public one,
//! with a config file specifying chain = "path" under [ckb].
//!
//! Because the limitation of toml library,
//! we must put nested config struct in the tail to make it serializable,
//! details https://docs.rs/toml/0.5.0/toml/ser/index.html

use crate::consensus::{
    build_genesis_dao_data, build_genesis_epoch_ext, Consensus, ConsensusBuilder,
    SATOSHI_CELL_OCCUPIED_RATIO, SATOSHI_PUBKEY_HASH, TYPE_ID_CODE_HASH,
};
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
        capacity_bytes, BlockBuilder, BlockNumber, BlockView, Capacity, Cycle, EpochNumber,
        EpochNumberWithFraction, Ratio, ScriptHashType, TransactionBuilder, TransactionView,
    },
    h256, packed,
    prelude::*,
    H160, H256, U128,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::TryFrom;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

pub use error::SpecError;

pub mod consensus;
mod error;

// Just a random secp256k1 secret key for dep group input cell's lock
const SPECIAL_CELL_PRIVKEY: H256 =
    h256!("0xd0c5c1e2d5af8b6ced3c0800937f996c1fa38c29186cade0cd8b5a73c97aaca3");

/// TODO(doc): @zhangsoledad
pub const OUTPUT_INDEX_SECP256K1_BLAKE160_SIGHASH_ALL: u64 = 1;
/// TODO(doc): @zhangsoledad
pub const OUTPUT_INDEX_DAO: u64 = 2;
/// TODO(doc): @zhangsoledad
pub const OUTPUT_INDEX_SECP256K1_DATA: u64 = 3;
/// TODO(doc): @zhangsoledad
pub const OUTPUT_INDEX_SECP256K1_BLAKE160_MULTISIG_ALL: u64 = 4;

/// TODO(doc): @zhangsoledad
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ChainSpec {
    /// TODO(doc): @zhangsoledad
    pub name: String,
    /// TODO(doc): @zhangsoledad
    pub genesis: Genesis,
    /// TODO(doc): @zhangsoledad
    #[serde(default)]
    pub params: Params,
    /// TODO(doc): @zhangsoledad
    pub pow: Pow,
}

/// TODO(doc): @zhangsoledad
pub mod default_params {
    use crate::consensus::{
        CELLBASE_MATURITY, DEFAULT_EPOCH_DURATION_TARGET,
        DEFAULT_PRIMARY_EPOCH_REWARD_HALVING_INTERVAL, DEFAULT_SECONDARY_EPOCH_REWARD,
        GENESIS_EPOCH_LENGTH, INITIAL_PRIMARY_EPOCH_REWARD, MAX_BLOCK_BYTES, MAX_BLOCK_CYCLES,
        MAX_BLOCK_PROPOSALS_LIMIT,
    };
    use ckb_types::core::{Capacity, Cycle, EpochNumber};

    /// TODO(doc): @zhangsoledad
    pub fn initial_primary_epoch_reward() -> Capacity {
        INITIAL_PRIMARY_EPOCH_REWARD
    }

    /// TODO(doc): @zhangsoledad
    pub fn secondary_epoch_reward() -> Capacity {
        DEFAULT_SECONDARY_EPOCH_REWARD
    }

    /// TODO(doc): @zhangsoledad
    pub fn max_block_cycles() -> Cycle {
        MAX_BLOCK_CYCLES
    }

    /// TODO(doc): @zhangsoledad
    pub fn max_block_bytes() -> u64 {
        MAX_BLOCK_BYTES
    }

    /// TODO(doc): @zhangsoledad
    pub fn cellbase_maturity() -> u64 {
        CELLBASE_MATURITY.full_value()
    }

    /// TODO(doc): @zhangsoledad
    pub fn primary_epoch_reward_halving_interval() -> EpochNumber {
        DEFAULT_PRIMARY_EPOCH_REWARD_HALVING_INTERVAL
    }

    /// TODO(doc): @zhangsoledad
    pub fn epoch_duration_target() -> u64 {
        DEFAULT_EPOCH_DURATION_TARGET
    }

    /// TODO(doc): @zhangsoledad
    pub fn genesis_epoch_length() -> u64 {
        GENESIS_EPOCH_LENGTH
    }

    /// TODO(doc): @zhangsoledad
    pub fn max_block_proposals_limit() -> u64 {
        MAX_BLOCK_PROPOSALS_LIMIT
    }
}

/// TODO(doc): @zhangsoledad
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Params {
    /// TODO(doc): @zhangsoledad
    #[serde(default = "default_params::initial_primary_epoch_reward")]
    pub initial_primary_epoch_reward: Capacity,
    /// TODO(doc): @zhangsoledad
    #[serde(default = "default_params::secondary_epoch_reward")]
    pub secondary_epoch_reward: Capacity,
    /// TODO(doc): @zhangsoledad
    #[serde(default = "default_params::max_block_cycles")]
    pub max_block_cycles: Cycle,
    /// TODO(doc): @zhangsoledad
    #[serde(default = "default_params::max_block_bytes")]
    pub max_block_bytes: u64,
    /// TODO(doc): @zhangsoledad
    #[serde(default = "default_params::cellbase_maturity")]
    pub cellbase_maturity: u64,
    /// TODO(doc): @zhangsoledad
    #[serde(default = "default_params::primary_epoch_reward_halving_interval")]
    pub primary_epoch_reward_halving_interval: EpochNumber,
    /// TODO(doc): @zhangsoledad
    #[serde(default = "default_params::epoch_duration_target")]
    pub epoch_duration_target: u64,
    /// TODO(doc): @zhangsoledad
    #[serde(default = "default_params::genesis_epoch_length")]
    pub genesis_epoch_length: BlockNumber,
    /// TODO(doc): @zhangsoledad
    #[serde(default)]
    pub permanent_difficulty_in_dummy: bool,
    /// TODO(doc): @zhangsoledad
    #[serde(default = "default_params::max_block_proposals_limit")]
    pub max_block_proposals_limit: u64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            initial_primary_epoch_reward: default_params::initial_primary_epoch_reward(),
            secondary_epoch_reward: default_params::secondary_epoch_reward(),
            max_block_cycles: default_params::max_block_cycles(),
            max_block_bytes: default_params::max_block_bytes(),
            cellbase_maturity: default_params::cellbase_maturity(),
            primary_epoch_reward_halving_interval:
                default_params::primary_epoch_reward_halving_interval(),
            epoch_duration_target: default_params::epoch_duration_target(),
            genesis_epoch_length: default_params::genesis_epoch_length(),
            permanent_difficulty_in_dummy: false,
            max_block_proposals_limit: default_params::max_block_proposals_limit(),
        }
    }
}

/// TODO(doc): @zhangsoledad
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Genesis {
    /// TODO(doc): @zhangsoledad
    pub version: u32,
    /// TODO(doc): @zhangsoledad
    pub parent_hash: H256,
    /// TODO(doc): @zhangsoledad
    pub timestamp: u64,
    /// TODO(doc): @zhangsoledad
    pub compact_target: u32,
    /// TODO(doc): @zhangsoledad
    pub uncles_hash: H256,
    /// TODO(doc): @zhangsoledad
    pub hash: Option<H256>,
    /// TODO(doc): @zhangsoledad
    pub nonce: U128,
    /// TODO(doc): @zhangsoledad
    pub issued_cells: Vec<IssuedCell>,
    /// TODO(doc): @zhangsoledad
    pub genesis_cell: GenesisCell,
    /// TODO(doc): @zhangsoledad
    pub system_cells: Vec<SystemCell>,
    /// TODO(doc): @zhangsoledad
    pub system_cells_lock: Script,
    /// TODO(doc): @zhangsoledad
    pub bootstrap_lock: Script,
    /// TODO(doc): @zhangsoledad
    pub dep_groups: Vec<DepGroupResource>,
    /// TODO(doc): @zhangsoledad
    #[serde(default)]
    pub satoshi_gift: SatoshiGift,
}

/// TODO(doc): @zhangsoledad
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SystemCell {
    /// TODO(doc): @zhangsoledad
    // NOTE: must put `create_type_id` before `file` otherwise this struct can not serialize
    pub create_type_id: bool,
    /// TODO(doc): @zhangsoledad
    // Overwrite the cell capacity. Set to None to use the minimal capacity.
    pub capacity: Option<u64>,
    /// TODO(doc): @zhangsoledad
    pub file: Resource,
}

/// TODO(doc): @zhangsoledad
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct GenesisCell {
    /// TODO(doc): @zhangsoledad
    pub message: String,
    /// TODO(doc): @zhangsoledad
    pub lock: Script,
}

/// TODO(doc): @zhangsoledad
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct IssuedCell {
    /// TODO(doc): @zhangsoledad
    pub capacity: Capacity,
    /// TODO(doc): @zhangsoledad
    pub lock: Script,
}

/// TODO(doc): @zhangsoledad
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DepGroupResource {
    /// TODO(doc): @zhangsoledad
    pub name: String,
    /// TODO(doc): @zhangsoledad
    pub files: Vec<Resource>,
}

/// TODO(doc): @zhangsoledad
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SatoshiGift {
    /// TODO(doc): @zhangsoledad
    pub satoshi_pubkey_hash: H160,
    /// TODO(doc): @zhangsoledad
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

/// TODO(doc): @zhangsoledad
#[derive(Debug)]
pub enum SpecLoadError {
    /// TODO(doc): @zhangsoledad
    FileNotFound,
    /// TODO(doc): @zhangsoledad
    GenesisMismatch {
        /// TODO(doc): @zhangsoledad
        expect: H256,
        /// TODO(doc): @zhangsoledad
        actual: H256,
    },
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
    /// TODO(doc): @zhangsoledad
    pub fn load_from(resource: &Resource) -> Result<ChainSpec, Box<dyn Error>> {
        if !resource.exists() {
            return Err(SpecLoadError::file_not_found());
        }
        let config_bytes = resource.get()?;
        let mut spec: ChainSpec = toml::from_slice(&config_bytes)?;

        if let Some(parent) = resource.parent() {
            for r in spec.genesis.system_cells.iter_mut() {
                r.file.absolutize(parent)
            }
        }

        Ok(spec)
    }

    /// TODO(doc): @zhangsoledad
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

    /// TODO(doc): @zhangsoledad
    pub fn build_consensus(&self) -> Result<Consensus, Box<dyn Error>> {
        let genesis_epoch_ext = build_genesis_epoch_ext(
            self.params.initial_primary_epoch_reward,
            self.genesis.compact_target,
            self.params.genesis_epoch_length,
            self.params.epoch_duration_target,
        );
        let genesis_block = self.build_genesis()?;
        self.verify_genesis_hash(&genesis_block)?;

        let consensus = ConsensusBuilder::new(genesis_block, genesis_epoch_ext)
            .id(self.name.clone())
            .cellbase_maturity(EpochNumberWithFraction::from_full_value(
                self.params.cellbase_maturity,
            ))
            .secondary_epoch_reward(self.params.secondary_epoch_reward)
            .max_block_cycles(self.params.max_block_cycles)
            .max_block_bytes(self.params.max_block_bytes)
            .pow(self.pow.clone())
            .satoshi_pubkey_hash(self.genesis.satoshi_gift.satoshi_pubkey_hash.clone())
            .satoshi_cell_occupied_ratio(self.genesis.satoshi_gift.satoshi_cell_occupied_ratio)
            .primary_epoch_reward_halving_interval(
                self.params.primary_epoch_reward_halving_interval,
            )
            .initial_primary_epoch_reward(self.params.initial_primary_epoch_reward)
            .epoch_duration_target(self.params.epoch_duration_target)
            .permanent_difficulty_in_dummy(self.params.permanent_difficulty_in_dummy)
            .max_block_proposals_limit(self.params.max_block_proposals_limit)
            .build();

        Ok(consensus)
    }

    /// TODO(doc): @zhangsoledad
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

        let genesis_epoch_length = self.params.genesis_epoch_length;
        let genesis_primary_issuance = calculate_block_reward(
            self.params.initial_primary_epoch_reward,
            genesis_epoch_length,
        );
        let genesis_secondary_issuance =
            calculate_block_reward(self.params.secondary_epoch_reward, genesis_epoch_length);
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
            .uncles_hash(self.genesis.uncles_hash.pack())
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

/// TODO(doc): @zhangsoledad
pub fn build_genesis_type_id_script(output_index: u64) -> packed::Script {
    build_type_id_script(&packed::CellInput::new_cellbase_input(0), output_index)
}

/// TODO(doc): @zhangsoledad
pub fn build_type_id_script(input: &packed::CellInput, output_index: u64) -> packed::Script {
    let mut blake2b = new_blake2b();
    blake2b.update(&input.as_slice());
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

/// TODO(doc): @zhangsoledad
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

#[cfg(test)]
pub mod test {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;

    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct SystemCell {
        pub path: String,
        pub index: usize,
        pub data_hash: H256,
        pub type_hash: Option<H256>,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct DepGroups {
        pub included_cells: Vec<String>,
        pub tx_hash: H256,
        pub index: usize,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct SpecHashes {
        pub genesis: H256,
        pub cellbase: H256,
        pub system_cells: Vec<SystemCell>,
        pub dep_groups: Vec<DepGroups>,
    }

    fn load_spec_by_name(name: &str) -> ChainSpec {
        let res = if name == "ckb" {
            Resource::bundled("specs/mainnet.toml".to_string())
        } else {
            let base_name = &name[4..];
            Resource::bundled(format!("specs/{}.toml", base_name))
        };

        ChainSpec::load_from(&res).expect("load spec by name")
    }

    #[test]
    fn test_bundled_specs() {
        let bundled_spec_err: &str = r#"
            Unmatched Bundled Spec.

            Forget to generate docs/hashes.toml? Try to run;

                ckb list-hashes -b > docs/hashes.toml
        "#;

        let spec_hashes: HashMap<String, SpecHashes> =
            toml::from_str(include_str!("../../docs/hashes.toml")).unwrap();

        for (name, spec_hashes) in spec_hashes.iter() {
            let spec = load_spec_by_name(name);
            assert_eq!(name, &spec.name, "{}", bundled_spec_err);
            if let Some(genesis_hash) = &spec.genesis.hash {
                assert_eq!(genesis_hash, &spec_hashes.genesis, "{}", bundled_spec_err);
            }

            let consensus = spec.build_consensus();
            if let Err(err) = consensus {
                panic!("{}", err);
            }
            let consensus = consensus.unwrap();
            let block = consensus.genesis_block();
            let cellbase = block.transaction(0).unwrap();
            let cellbase_hash: H256 = cellbase.hash().unpack();

            assert_eq!(spec_hashes.cellbase, cellbase_hash);

            let mut system_cells = HashMap::new();
            for (index_minus_one, (cell, (output, data))) in spec_hashes
                .system_cells
                .iter()
                .zip(
                    cellbase
                        .outputs()
                        .into_iter()
                        .zip(cellbase.outputs_data().into_iter())
                        .skip(1),
                )
                .enumerate()
            {
                let data_hash: H256 = packed::CellOutput::calc_data_hash(&data.raw_data()).unpack();
                let type_hash: Option<H256> = output
                    .type_()
                    .to_opt()
                    .map(|script| script.calc_script_hash().unpack());
                assert_eq!(index_minus_one + 1, cell.index, "{}", bundled_spec_err);
                assert_eq!(cell.data_hash, data_hash, "{}", bundled_spec_err);
                assert_eq!(cell.type_hash, type_hash, "{}", bundled_spec_err);
                system_cells.insert(cell.index, cell.path.as_str());
            }

            // dep group tx should be the first tx except cellbase
            let dep_group_tx = block.transaction(1).unwrap();

            // input index of dep group tx
            let dep_group_tx_input_index = system_cells.len() + 1;
            let input_capacity: Capacity = cellbase
                .output(dep_group_tx_input_index)
                .unwrap()
                .capacity()
                .unpack();
            let outputs_capacity = dep_group_tx
                .outputs()
                .into_iter()
                .map(|output| Unpack::<Capacity>::unpack(&output.capacity()))
                .try_fold(Capacity::zero(), Capacity::safe_add)
                .unwrap();
            // capacity for input and outpus should be same
            assert_eq!(input_capacity, outputs_capacity);

            // dep group tx has only one input
            assert_eq!(dep_group_tx.inputs().len(), 1);

            // all dep groups should be in the spec file
            assert_eq!(
                dep_group_tx.outputs_data().len(),
                spec_hashes.dep_groups.len(),
                "{}",
                bundled_spec_err
            );

            for (i, output_data) in dep_group_tx.outputs_data().into_iter().enumerate() {
                let dep_group = &spec_hashes.dep_groups[i];

                // check the tx hashes of dep groups in spec file
                let tx_hash = dep_group.tx_hash.pack();
                assert_eq!(tx_hash, dep_group_tx.hash(), "{}", bundled_spec_err);

                let out_point_vec =
                    packed::OutPointVec::from_slice(&output_data.raw_data()).unwrap();

                // all cells included by a dep group should be list in the spec file
                assert_eq!(
                    out_point_vec.len(),
                    dep_group.included_cells.len(),
                    "{}",
                    bundled_spec_err
                );

                for (j, out_point) in out_point_vec.into_iter().enumerate() {
                    let dep_path = &dep_group.included_cells[j];

                    // dep groups out_point should point to cellbase
                    assert_eq!(cellbase.hash(), out_point.tx_hash(), "{}", bundled_spec_err);

                    let index_in_cellbase: usize = out_point.index().unpack();

                    // check index for included cells in dep groups
                    assert_eq!(
                        system_cells[&index_in_cellbase], dep_path,
                        "{}",
                        bundled_spec_err
                    );
                }
            }
        }
    }

    #[test]
    fn test_default_params() {
        let params: Params = toml::from_str("").unwrap();
        let expected = Params::default();
        assert_eq!(params, expected);

        let test_params: &str = r#"
            genesis_epoch_length = 100
        "#;

        let params: Params = toml::from_str(&test_params).unwrap();
        let mut expected = Params::default();
        expected.genesis_epoch_length = 100;

        assert_eq!(params, expected);

        let test_params: &str = r#"
            max_block_bytes = 100
        "#;

        let params: Params = toml::from_str(&test_params).unwrap();
        let mut expected = Params::default();
        expected.max_block_bytes = 100;

        assert_eq!(params, expected);

        let test_params: &str = r#"
            max_block_proposals_limit = 100
        "#;

        let params: Params = toml::from_str(&test_params).unwrap();
        let mut expected = Params::default();
        expected.max_block_proposals_limit = 100;

        assert_eq!(params, expected);
    }
}
