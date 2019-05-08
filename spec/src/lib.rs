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
use ckb_core::block::Block;
use ckb_core::block::BlockBuilder;
use ckb_core::extras::EpochExt;
use ckb_core::header::HeaderBuilder;
use ckb_core::transaction::{CellInput, CellOutput, Transaction, TransactionBuilder};
use ckb_core::{BlockNumber, Bytes, Capacity, Cycle};
use ckb_pow::{Pow, PowEngine};
use ckb_resource::{Resource, ResourceLocator};
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use occupied_capacity::OccupiedCapacity;
use serde_derive::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

pub mod consensus;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChainSpec {
    pub resource: Resource,
    pub name: String,
    pub genesis: Genesis,
    pub params: Params,
    pub system_cells: Vec<Resource>,
    pub pow: Pow,
}

// change the order will break integration test, see module doc.
#[derive(Serialize, Deserialize)]
pub struct ChainSpecConfig {
    pub name: String,
    pub genesis: Genesis,
    pub params: Params,
    pub system_cells: Vec<SystemCell>,
    pub pow: Pow,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Params {
    pub epoch_reward: Capacity,
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
    pub seal: Seal,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Seal {
    pub nonce: u64,
    pub proof: Bytes,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SystemCell {
    pub path: PathBuf,
}

#[derive(Debug)]
pub struct FileNotFoundError;

impl FileNotFoundError {
    fn boxed() -> Box<Self> {
        Box::new(FileNotFoundError)
    }
}

impl Error for FileNotFoundError {}

impl fmt::Display for FileNotFoundError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ChainSpec: file not found")
    }
}

pub struct GenesisError {
    expect: H256,
    actual: H256,
}

impl GenesisError {
    fn boxed(self) -> Box<Self> {
        Box::new(self)
    }
}

impl Error for GenesisError {}

impl fmt::Debug for GenesisError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "GenesisError: hash mismatch, expect {:x}, actual {:x}",
            self.expect, self.actual
        )
    }
}

impl fmt::Display for GenesisError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl ChainSpec {
    pub fn resolve_relative_to(
        locator: &ResourceLocator,
        spec_path: PathBuf,
        config_file: &Resource,
    ) -> Result<ChainSpec, Box<Error>> {
        let resource = match locator.resolve_relative_to(spec_path, config_file) {
            Some(r) => r,
            None => return Err(FileNotFoundError::boxed()),
        };
        let config_bytes = resource.get()?;
        let spec_config: ChainSpecConfig = toml::from_slice(&config_bytes)?;

        let system_cells_result: Result<Vec<_>, FileNotFoundError> = spec_config
            .system_cells
            .into_iter()
            .map(|c| {
                locator
                    .resolve_relative_to(c.path, &resource)
                    .ok_or(FileNotFoundError)
            })
            .collect();

        Ok(ChainSpec {
            resource,
            system_cells: system_cells_result?,
            name: spec_config.name,
            genesis: spec_config.genesis,
            params: spec_config.params,
            pow: spec_config.pow,
        })
    }

    pub fn pow_engine(&self) -> Arc<dyn PowEngine> {
        self.pow.engine()
    }

    fn build_system_cells_transaction(&self) -> Result<Transaction, Box<Error>> {
        let outputs_result: Result<Vec<_>, _> = self
            .system_cells
            .iter()
            .map(|c| {
                c.get()
                    .map_err(|err| Box::new(err) as Box<Error>)
                    .and_then(|data| {
                        // TODO: we should provide a proper lock script here so system cells
                        // can be updated.
                        let mut cell = CellOutput::default();
                        cell.data = data.into_owned().into();
                        cell.capacity = cell.occupied_capacity()?;
                        Ok(cell)
                    })
            })
            .collect();

        let outputs = outputs_result?;
        Ok(TransactionBuilder::default()
            .outputs(outputs)
            .input(CellInput::new_cellbase_input(0))
            .build())
    }

    fn verify_genesis_hash(&self, genesis: &Block) -> Result<(), Box<Error>> {
        if let Some(ref expect) = self.genesis.hash {
            let actual = genesis.header().hash();
            if actual != expect {
                return Err(GenesisError {
                    actual: actual.clone(),
                    expect: expect.clone(),
                }
                .boxed());
            }
        }
        Ok(())
    }

    pub fn to_consensus(&self) -> Result<Consensus, Box<Error>> {
        let header_builder = HeaderBuilder::default()
            .version(self.genesis.version)
            .parent_hash(self.genesis.parent_hash.clone())
            .timestamp(self.genesis.timestamp)
            .difficulty(self.genesis.difficulty.clone())
            .nonce(self.genesis.seal.nonce)
            .proof(self.genesis.seal.proof.clone())
            .uncles_hash(self.genesis.uncles_hash.clone());

        let genesis_block = BlockBuilder::from_header_builder(header_builder)
            .transaction(self.build_system_cells_transaction()?)
            .build();

        self.verify_genesis_hash(&genesis_block)?;

        let block_reward =
            Capacity::shannons(self.params.epoch_reward.as_u64() / GENESIS_EPOCH_LENGTH);
        let remainder_reward =
            Capacity::shannons(self.params.epoch_reward.as_u64() / GENESIS_EPOCH_LENGTH);

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
            .set_max_block_cycles(self.params.max_block_cycles)
            .set_pow(self.pow.clone());

        Ok(consensus)
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use ckb_core::script::Script;
    use serde_derive::{Deserialize, Serialize};
    use std::collections::HashMap;

    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct SystemCellHashes {
        pub path: String,
        pub code_hash: H256,
        pub script_hash: H256,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct SpecHashes {
        pub genesis: H256,
        pub system_cells_transaction: H256,
        pub system_cells: Vec<SystemCellHashes>,
    }

    fn load_spec_by_name(name: &str) -> ChainSpec {
        let spec_path = match name {
            "ckb_dev" => PathBuf::from("specs/dev.toml"),
            "ckb_testnet" => PathBuf::from("specs/testnet.toml"),
            _ => panic!("Unknown spec name {}", name),
        };

        let locator = ResourceLocator::current_dir().unwrap();
        let ckb = Resource::Bundled("ckb.toml".to_string());
        ChainSpec::resolve_relative_to(&locator, spec_path, &ckb).expect("load spec by name")
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

            let consensus = spec.to_consensus().expect("spec to consensus");
            let block = consensus.genesis_block();
            let cells_tx = &block.transactions()[0];

            assert_eq!(&spec_hashes.system_cells_transaction, cells_tx.hash());

            for (output, cell_hashes) in cells_tx
                .outputs()
                .iter()
                .zip(spec_hashes.system_cells.iter())
            {
                let code_hash = output.data_hash();
                let script_hash = Script::new(vec![], code_hash.clone()).hash();
                assert_eq!(cell_hashes.code_hash, code_hash, "{}", bundled_spec_err);
                assert_eq!(cell_hashes.script_hash, script_hash, "{}", bundled_spec_err);
            }
        }
    }
}
