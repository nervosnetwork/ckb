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

use crate::consensus::{Consensus, GENESIS_EPOCH_LENGTH};
use ckb_core::{
    block::{Block, BlockBuilder},
    extras::EpochExt,
    header::HeaderBuilder,
    script::Script as CoreScript,
    transaction::{CellInput, CellOutput, Transaction, TransactionBuilder},
    BlockNumber, Bytes, Capacity, Cycle,
};
use ckb_dao_utils::genesis_dao_data;
use ckb_jsonrpc_types::Script;
use ckb_pow::{Pow, PowEngine};
use ckb_resource::Resource;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use serde_derive::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::sync::Arc;

pub mod consensus;

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
    pub issued_cells: Vec<IssuedCell>,
    pub genesis_cell: GenesisCell,
    pub system_cells: SystemCells,
    pub bootstrap_lock: Script,
    pub seal: Seal,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Seal {
    pub nonce: u64,
    pub proof: Bytes,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SystemCells {
    pub files: Vec<Resource>,
    pub lock: Script,
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
            for r in spec.genesis.system_cells.files.iter_mut() {
                r.absolutize(parent)
            }
        }

        Ok(spec)
    }

    pub fn pow_engine(&self) -> Arc<dyn PowEngine> {
        self.pow.engine()
    }

    fn verify_genesis_hash(&self, genesis: &Block) -> Result<(), Box<dyn Error>> {
        if let Some(ref expect) = self.genesis.hash {
            let actual = genesis.header().hash();
            if actual != expect {
                return Err(SpecLoadError::genesis_mismatch(
                    expect.clone(),
                    actual.clone(),
                ));
            }
        }
        Ok(())
    }

    pub fn build_consensus(&self) -> Result<Consensus, Box<dyn Error>> {
        let genesis_block = self.genesis.build_block()?;
        self.verify_genesis_hash(&genesis_block)?;

        let block_reward =
            Capacity::shannons(self.params.epoch_reward.as_u64() / GENESIS_EPOCH_LENGTH);
        let remainder_reward =
            Capacity::shannons(self.params.epoch_reward.as_u64() % GENESIS_EPOCH_LENGTH);

        let genesis_epoch_ext = EpochExt::new(
            0,                        // number
            block_reward,             // block_reward
            remainder_reward,         // remainder_reward
            H256::zero(),             // last_block_hash_in_previous_epoch
            0,                        // start
            GENESIS_EPOCH_LENGTH,     // length
            genesis_block.header().difficulty().clone() // difficulty,
        );

        let consensus = Consensus::default()
            .set_id(self.name.clone())
            .set_genesis_epoch_ext(genesis_epoch_ext)
            .set_genesis_block(genesis_block)
            .set_cellbase_maturity(self.params.cellbase_maturity)
            .set_epoch_reward(self.params.epoch_reward)
            .set_secondary_epoch_reward(self.params.secondary_epoch_reward)
            .set_max_block_cycles(self.params.max_block_cycles)
            .set_pow(self.pow.clone());

        Ok(consensus)
    }
}

impl Genesis {
    fn build_block(&self) -> Result<Block, Box<dyn Error>> {
        let cellbase_transaction = self.build_cellbase_transaction()?;
        let dao = genesis_dao_data(&cellbase_transaction)?;

        let header_builder = HeaderBuilder::default()
            .version(self.version)
            .parent_hash(self.parent_hash.clone())
            .timestamp(self.timestamp)
            .difficulty(self.difficulty.clone())
            .nonce(self.seal.nonce)
            .proof(self.seal.proof.clone())
            .uncles_hash(self.uncles_hash.clone())
            .dao(dao);

        Ok(BlockBuilder::from_header_builder(header_builder)
            .transaction(cellbase_transaction)
            .build())
    }

    fn build_cellbase_transaction(&self) -> Result<Transaction, Box<dyn Error>> {
        let mut outputs =
            Vec::<CellOutput>::with_capacity(1 + self.system_cells.len() + self.issued_cells.len());

        // Layout of genesis cellbase:
        // - genesis cell, which contains a message and can never be spent.
        // - system cells, which stores the built-in code blocks.
        // - issued cells
        outputs.push(self.genesis_cell.build_output()?);
        self.system_cells.build_outputs_into(&mut outputs)?;
        outputs.extend(self.issued_cells.iter().map(IssuedCell::build_output));

        Ok(TransactionBuilder::default()
            .outputs(outputs)
            .input(CellInput::new_cellbase_input(0))
            .witness(CoreScript::from(self.bootstrap_lock.clone()).into_witness())
            .build())
    }
}

impl GenesisCell {
    fn build_output(&self) -> Result<CellOutput, Box<dyn Error>> {
        let mut cell = CellOutput::default();
        cell.data = self.message.as_bytes().into();
        cell.lock = self.lock.clone().into();
        cell.capacity = cell.occupied_capacity()?;
        Ok(cell)
    }
}

impl IssuedCell {
    fn build_output(&self) -> CellOutput {
        let mut cell = CellOutput::default();
        cell.lock = self.lock.clone().into();
        cell.capacity = self.capacity;
        cell
    }
}

impl SystemCells {
    fn len(&self) -> usize {
        self.files.len()
    }

    fn build_outputs_into(&self, outputs: &mut Vec<CellOutput>) -> Result<(), Box<dyn Error>> {
        for res in &self.files {
            let data = res.get()?;
            let mut cell = CellOutput::default();
            cell.data = data.into_owned().into();
            cell.lock = self.lock.clone().into();
            cell.capacity = cell.occupied_capacity()?;
            outputs.push(cell);
        }

        Ok(())
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
        pub code_hash: H256,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct SpecHashes {
        pub genesis: H256,
        pub cellbase: H256,
        pub system_cells: Vec<SystemCell>,
    }

    fn load_spec_by_name(name: &str) -> ChainSpec {
        let res = match name {
            "ckb_dev" => Resource::bundled("specs/dev.toml".to_string()),
            "ckb_testnet" => Resource::bundled("specs/testnet.toml".to_string()),
            _ => panic!("Unknown spec name {}", name),
        };

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
            let cellbase = &block.transactions()[0];

            assert_eq!(&spec_hashes.cellbase, cellbase.hash());

            for (index_minus_one, (output, cell)) in cellbase
                .outputs()
                .iter()
                .skip(1)
                .zip(spec_hashes.system_cells.iter())
                .enumerate()
            {
                let code_hash = output.data_hash();
                assert_eq!(index_minus_one + 1, cell.index, "{}", bundled_spec_err);
                assert_eq!(cell.code_hash, code_hash, "{}", bundled_spec_err);
            }
        }
    }
}
