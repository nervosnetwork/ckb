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
    SATOSHI_CELL_OCCUPIED_RATIO, SATOSHI_PUBKEY_HASH,
};
use ckb_crypto::secp::Privkey;
use ckb_hash::{blake2b_256, new_blake2b};
use ckb_jsonrpc_types::Script;
use ckb_pow::{Pow, PowEngine};
use ckb_resource::{
    Resource, CODE_HASH_DAO, CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL, CODE_HASH_SECP256K1_DATA,
    CODE_HASH_SECP256K1_RIPEMD160_SHA256_SIGHASH_ALL,
};
use ckb_types::{
    bytes::Bytes,
    constants::TYPE_ID_CODE_HASH,
    core::{
        capacity_bytes, BlockBuilder, BlockNumber, BlockView, Capacity, Cycle, EpochNumber,
        EpochNumberWithFraction, Ratio, ScriptHashType, TransactionBuilder, TransactionView,
    },
    h256, packed,
    prelude::*,
    H160, H256,
};
pub use error::SpecError;
use serde_derive::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fmt;
use std::sync::Arc;

pub mod consensus;
mod error;

// Just a random secp256k1 secret key for dep group input cell's lock
const SPECIAL_CELL_PRIVKEY: H256 =
    h256!("0xd0c5c1e2d5af8b6ced3c0800937f996c1fa38c29186cade0cd8b5a73c97aaca3");
const SPECIAL_CELL_CAPACITY: Capacity = capacity_bytes!(500);

pub const OUTPUT_INDEX_SECP256K1_BLAKE160_SIGHASH_ALL: u64 = 1;
pub const OUTPUT_INDEX_DAO: u64 = 2;
pub const OUTPUT_INDEX_SECP256K1_DATA: u64 = 3;
pub const OUTPUT_INDEX_SECP256K1_RIPEMD160_SHA256_SIGHASH_ALL: u64 = 4;

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ChainSpec {
    pub name: String,
    pub genesis: Genesis,
    #[serde(default)]
    pub params: Params,
    pub pow: Pow,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Params {
    pub initial_primary_epoch_reward: Capacity,
    pub secondary_epoch_reward: Capacity,
    pub max_block_cycles: Cycle,
    pub cellbase_maturity: u64,
    pub primary_epoch_reward_halving_interval: EpochNumber,
    pub epoch_duration_target: u64,
    pub genesis_epoch_length: BlockNumber,
}

impl Default for Params {
    fn default() -> Self {
        use crate::consensus::{
            CELLBASE_MATURITY, DEFAULT_EPOCH_DURATION_TARGET,
            DEFAULT_PRIMARY_EPOCH_REWARD_HALVING_INTERVAL, DEFAULT_SECONDARY_EPOCH_REWARD,
            GENESIS_EPOCH_LENGTH, INITIAL_PRIMARY_EPOCH_REWARD, MAX_BLOCK_CYCLES,
        };
        Params {
            initial_primary_epoch_reward: INITIAL_PRIMARY_EPOCH_REWARD,
            secondary_epoch_reward: DEFAULT_SECONDARY_EPOCH_REWARD,
            max_block_cycles: MAX_BLOCK_CYCLES,
            cellbase_maturity: CELLBASE_MATURITY.full_value(),
            primary_epoch_reward_halving_interval: DEFAULT_PRIMARY_EPOCH_REWARD_HALVING_INTERVAL,
            epoch_duration_target: DEFAULT_EPOCH_DURATION_TARGET,
            genesis_epoch_length: GENESIS_EPOCH_LENGTH,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Genesis {
    pub version: u32,
    pub parent_hash: H256,
    pub timestamp: u64,
    pub compact_target: u32,
    pub uncles_hash: H256,
    pub hash: Option<H256>,
    pub nonce: u128,
    pub issued_cells: Vec<IssuedCell>,
    pub genesis_cell: GenesisCell,
    pub system_cells: Vec<SystemCell>,
    pub system_cells_lock: Script,
    pub bootstrap_lock: Script,
    pub dep_groups: BTreeMap<String, Vec<Resource>>,
    #[serde(default)]
    pub satoshi_gift: SatoshiGift,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SystemCell {
    // NOTE: must put `create_type_id` before `file` otherwise this struct can not serialize
    pub create_type_id: bool,
    pub file: Resource,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct GenesisCell {
    pub message: String,
    pub lock: Script,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct IssuedCell {
    pub capacity: Capacity,
    pub lock: Script,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SatoshiGift {
    pub satoshi_pubkey_hash: H160,
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
pub enum SpecLoadError {
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
            .pow(self.pow.clone())
            .satoshi_pubkey_hash(self.genesis.satoshi_gift.satoshi_pubkey_hash.clone())
            .satoshi_cell_occupied_ratio(self.genesis.satoshi_gift.satoshi_cell_occupied_ratio)
            .primary_epoch_reward_halving_interval(
                self.params.primary_epoch_reward_halving_interval,
            )
            .initial_primary_epoch_reward(self.params.initial_primary_epoch_reward)
            .epoch_duration_target(self.params.epoch_duration_target)
            .build();

        Ok(consensus)
    }

    fn build_genesis(&self) -> Result<BlockView, Box<dyn Error>> {
        let cellbase_transaction = self.build_cellbase_transaction()?;
        // build transaction other than cellbase should return inputs for dao statistics
        let dep_group_transaction = self.build_dep_group_transaction(&cellbase_transaction)?;
        let dao = build_genesis_dao_data(
            vec![&cellbase_transaction, &dep_group_transaction],
            &self.genesis.satoshi_gift.satoshi_pubkey_hash,
            self.genesis.satoshi_gift.satoshi_cell_occupied_ratio,
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
        // Check lock scripts
        for lock_script in block
            .transactions()
            .into_iter()
            .flat_map(|tx| tx.outputs().into_iter().map(move |output| output.lock()))
            .filter(|lock_script| lock_script != &genesis_cell_lock)
        {
            match lock_script.hash_type().unpack() {
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
            OUTPUT_INDEX_SECP256K1_RIPEMD160_SHA256_SIGHASH_ALL as usize,
            &CODE_HASH_SECP256K1_RIPEMD160_SHA256_SIGHASH_ALL,
        )?;

        Ok(())
    }

    fn build_cellbase_transaction(&self) -> Result<TransactionView, Box<dyn Error>> {
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
            .hash_type(ScriptHashType::Data.pack())
            .build();
        let special_issued_cell = packed::CellOutput::new_builder()
            .capacity(SPECIAL_CELL_CAPACITY.pack())
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
            .values()
            .map(|files| {
                let out_points: Vec<_> = files
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

                let data = Bytes::from(out_points.pack().as_slice());
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
                .out_point(secp_data_out_point.clone())
                .build(),
            packed::CellDep::new_builder()
                .out_point(secp_blake160_out_point.clone())
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
        let data: Bytes = self.message.as_bytes().into();
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
        let cell = packed::CellOutput::new_builder()
            .type_(type_script.pack())
            .lock(lock.clone().into())
            .build_exact_capacity(Capacity::bytes(data.len())?)?;
        Ok((cell, data))
    }
}

fn secp_lock_arg(privkey: &Privkey) -> Bytes {
    let pubkey_data = privkey.pubkey().expect("Get pubkey failed").serialize();
    Bytes::from(&blake2b_256(&pubkey_data)[0..20])
}

pub fn build_genesis_type_id_script(output_index: u64) -> packed::Script {
    build_type_id_script(&packed::CellInput::new_cellbase_input(0), output_index)
}

pub fn build_type_id_script(input: &packed::CellInput, output_index: u64) -> packed::Script {
    let mut blake2b = new_blake2b();
    blake2b.update(&input.as_slice());
    blake2b.update(&output_index.to_le_bytes());
    let mut ret = [0; 32];
    blake2b.finalize(&mut ret);
    let script_arg = Bytes::from(&ret[..]);
    packed::Script::new_builder()
        .code_hash(TYPE_ID_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.pack())
        .args(script_arg.pack())
        .build()
}

#[cfg(test)]
pub mod test {
    use super::*;
    use serde_derive::{Deserialize, Serialize};
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
        // remove "ckb_" prefix
        let base_name = &name[4..];
        let res = Resource::bundled(format!("specs/{}.toml", base_name));
        ChainSpec::load_from(&res).expect("load spec by name")
    }

    #[test]
    fn test_bundled_specs() {
        let bundled_spec_err: &str = r#"
            Unmatched Bundled Spec.

            Forget to generate docs/hashes.toml? Try to run;

                ckb cli hashes -b > docs/hashes.toml
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
}
