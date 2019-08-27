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

use crate::consensus::Consensus;
use ckb_crypto::secp::Privkey;
use ckb_dao_utils::genesis_dao_data;
use ckb_hash::{blake2b_256, new_blake2b};
use ckb_jsonrpc_types::Script;
use ckb_pow::{Pow, PowEngine};
use ckb_resource::{Resource, CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL, CODE_HASH_SECP256K1_DATA};
use ckb_types::{
    bytes::Bytes,
    constants::TYPE_ID_CODE_HASH,
    core::{
        capacity_bytes, BlockBuilder, BlockNumber, BlockView, Capacity, Cycle, ScriptHashType,
        TransactionBuilder, TransactionView,
    },
    h256, packed,
    prelude::*,
    H256, U256,
};
use serde_derive::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

pub mod consensus;

// Just a random secp256k1 secret key for dep group input cell's lock
const SPECIAL_CELL_PRIVKEY: H256 =
    h256!("0xd0c5c1e2d5af8b6ced3c0800937f996c1fa38c29186cade0cd8b5a73c97aaca3");
const SPECIAL_CELL_CAPACITY: Capacity = capacity_bytes!(500);

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ChainSpec {
    pub name: String,
    pub genesis: Genesis,
    pub params: Params,
    pub pow: Pow,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Params {
    pub epoch_reward: Capacity,
    pub secondary_epoch_reward: Capacity,
    pub max_block_cycles: Cycle,
    pub cellbase_maturity: BlockNumber,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Genesis {
    pub version: u32,
    pub parent_hash: H256,
    pub timestamp: u64,
    pub difficulty: U256,
    pub uncles_hash: H256,
    pub hash: Option<H256>,
    pub nonce: u64,
    pub issued_cells: Vec<IssuedCell>,
    pub genesis_cell: GenesisCell,
    pub system_cells: Vec<SystemCell>,
    pub system_cells_lock: Script,
    pub bootstrap_lock: Script,
    pub dep_groups: BTreeMap<String, Vec<Resource>>,
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

#[derive(Debug)]
pub enum SpecLoadError {
    FileNotFound,
    ChainNameNotAllowed(String),
    GenesisMismatch { expect: H256, actual: H256 },
}

impl SpecLoadError {
    fn file_not_found() -> Box<Self> {
        Box::new(SpecLoadError::FileNotFound)
    }

    fn chain_name_not_allowed(name: String) -> Box<Self> {
        Box::new(SpecLoadError::ChainNameNotAllowed(name))
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
            SpecLoadError::ChainNameNotAllowed(name) => write!(
                f,
                "ChainSpec: name not allowed, expect ckb_dev, actual {}",
                name
            ),
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
        if !(resource.is_bundled() || spec.name == "ckb_dev" || spec.name == "ckb_integration_test")
        {
            return Err(SpecLoadError::chain_name_not_allowed(spec.name.clone()));
        }

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
        let genesis_block = self.genesis.build_block()?;
        self.verify_genesis_hash(&genesis_block)?;

        let consensus = Consensus::new(genesis_block, self.params.epoch_reward)
            .set_id(self.name.clone())
            .set_cellbase_maturity(self.params.cellbase_maturity)
            .set_secondary_epoch_reward(self.params.secondary_epoch_reward)
            .set_max_block_cycles(self.params.max_block_cycles)
            .set_pow(self.pow.clone());

        Ok(consensus)
    }
}

impl Genesis {
    fn build_block(&self) -> Result<BlockView, Box<dyn Error>> {
        let cellbase_transaction = self.build_cellbase_transaction()?;
        let dao = genesis_dao_data(&cellbase_transaction)?;
        let dep_group_transaction = self.build_dep_group_transaction(&cellbase_transaction)?;

        let block = BlockBuilder::default()
            .version(self.version.pack())
            .parent_hash(self.parent_hash.pack())
            .timestamp(self.timestamp.pack())
            .difficulty(self.difficulty.pack())
            .uncles_hash(self.uncles_hash.pack())
            .dao(dao)
            .nonce(self.nonce.pack())
            .transaction(cellbase_transaction)
            .transaction(dep_group_transaction)
            .build();
        Ok(block)
    }

    fn build_cellbase_transaction(&self) -> Result<TransactionView, Box<dyn Error>> {
        let input = packed::CellInput::new_cellbase_input(0);
        let mut outputs = Vec::<packed::CellOutput>::with_capacity(
            1 + self.system_cells.len() + self.issued_cells.len(),
        );
        let mut outputs_data = Vec::with_capacity(outputs.capacity());

        // Layout of genesis cellbase:
        // - genesis cell, which contains a message and can never be spent.
        // - system cells, which stores the built-in code blocks.
        // - special issued cell, for dep group cell in next transaction
        // - issued cells
        let (output, data) = self.genesis_cell.build_output()?;
        outputs.push(output);
        outputs_data.push(data);

        // The first output cell is genesis cell
        let system_cells_output_index_start = 1;
        let (system_cells_outputs, system_cells_data): (Vec<_>, Vec<_>) = self
            .system_cells
            .iter()
            .enumerate()
            .map(|(index, system_cell)| {
                system_cell.build_output(
                    &input,
                    system_cells_output_index_start + index as u64,
                    &self.system_cells_lock,
                )
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .unzip();
        outputs.extend(system_cells_outputs);
        outputs_data.extend(system_cells_data);

        let special_issued_lock = packed::Script::new_builder()
            .args(vec![secp_lock_arg(&Privkey::from(SPECIAL_CELL_PRIVKEY.clone()))].pack())
            .code_hash(CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL.clone().pack())
            .hash_type(ScriptHashType::Data.pack())
            .build();
        let special_issued_cell = packed::CellOutput::new_builder()
            .capacity(SPECIAL_CELL_CAPACITY.pack())
            .lock(special_issued_lock)
            .build();
        outputs.push(special_issued_cell);
        outputs_data.push(Bytes::new());

        outputs.extend(self.issued_cells.iter().map(IssuedCell::build_output));
        outputs_data.extend(self.issued_cells.iter().map(|_| Bytes::new()));

        let script: packed::Script = self.bootstrap_lock.clone().into();

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
            data_hash: &H256,
        ) -> Option<packed::OutPoint> {
            tx.outputs_data()
                .into_iter()
                .position(|data| {
                    let hash: H256 = packed::CellOutput::calc_data_hash(&data.raw_data());
                    &hash == data_hash
                })
                .map(|index| packed::OutPoint::new(tx.hash().clone().unpack(), index as u32))
        }

        let (outputs, outputs_data): (Vec<_>, Vec<_>) = self
            .dep_groups
            .values()
            .map(|files| {
                let out_points: Vec<_> = files
                    .iter()
                    .map(|res| {
                        let data: Bytes = res.get()?.into_owned().into();
                        let data_hash: H256 = packed::CellOutput::calc_data_hash(&data);
                        let out_point = find_out_point_by_data_hash(cellbase_tx, &data_hash)
                            .ok_or_else(|| {
                                format!("Can not find {} in genesis cellbase transaction", res)
                            })?;
                        Ok(out_point)
                    })
                    .collect::<Result<_, Box<dyn Error>>>()?;

                let data = Bytes::from(out_points.pack().as_slice());
                let cell = packed::CellOutput::new_builder()
                    .lock(self.system_cells_lock.clone().into())
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
            .position(|output| {
                output
                    .lock()
                    .args()
                    .get(0)
                    .map(|arg| arg.clone().unpack())
                    .as_ref()
                    == Some(&lock_arg)
            })
            .map(|index| packed::OutPoint::new(cellbase_tx.hash().clone().unpack(), index as u32))
            .expect("Get special issued input failed");
        let input = packed::CellInput::new(input_out_point, 0);

        let secp_data_out_point =
            find_out_point_by_data_hash(cellbase_tx, &CODE_HASH_SECP256K1_DATA)
                .ok_or_else(|| String::from("Get secp data out point failed"))?;
        let secp_blake160_out_point =
            find_out_point_by_data_hash(cellbase_tx, &CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL)
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
        let witness = vec![Bytes::from(sig.serialize()).pack()].pack();

        Ok(TransactionBuilder::default()
            .cell_deps(cell_deps)
            .input(input)
            .outputs(outputs)
            .outputs_data(outputs_data)
            .witness(witness)
            .build())
    }
}

fn secp_lock_arg(privkey: &Privkey) -> Bytes {
    let pubkey_data = privkey.pubkey().expect("Get pubkey failed").serialize();
    Bytes::from(&blake2b_256(&pubkey_data)[0..20])
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
            let mut blake2b = new_blake2b();
            blake2b.update(input.as_slice());
            blake2b.update(&output_index.to_le_bytes());
            let mut ret = [0; 32];
            blake2b.finalize(&mut ret);
            let script_arg = Bytes::from(&ret[..]).pack();
            let script = packed::Script::new_builder()
                .code_hash(TYPE_ID_CODE_HASH.pack())
                .hash_type(ScriptHashType::Type.pack())
                .args(vec![script_arg].pack())
                .build();
            Some(script)
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
    struct SpecHashes {
        pub genesis: H256,
        pub cellbase: H256,
        pub system_cells: Vec<SystemCell>,
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
            assert!(consensus.is_ok(), "{}", consensus.unwrap_err());
            let consensus = consensus.unwrap();
            let block = consensus.genesis_block();
            let cellbase = block.transaction(0).unwrap();
            let cellbase_hash: H256 = cellbase.hash().unpack();

            assert_eq!(spec_hashes.cellbase, cellbase_hash);

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
                let data_hash: H256 = packed::CellOutput::calc_data_hash(&data.raw_data());
                let type_hash: Option<H256> = output
                    .type_()
                    .to_opt()
                    .map(|script| script.calc_script_hash());
                assert_eq!(index_minus_one + 1, cell.index, "{}", bundled_spec_err);
                assert_eq!(cell.data_hash, data_hash, "{}", bundled_spec_err);
                assert_eq!(cell.type_hash, type_hash, "{}", bundled_spec_err);
            }
        }
    }
}
