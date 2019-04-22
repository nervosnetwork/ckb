//! # The Chain Specification
//!
//! By default, when simply running CKB, CKB will connect to the official public Nervos network.
//!
//! In order to run a chain different to the official public one,
//! with a config file specifying chain = "path" under [ckb].

use crate::consensus::Consensus;
use ckb_core::block::BlockBuilder;
use ckb_core::header::HeaderBuilder;
use ckb_core::script::Script;
use ckb_core::transaction::{CellOutput, Transaction, TransactionBuilder};
use ckb_core::{Capacity, Cycle};
use ckb_pow::{Pow, PowEngine};
use ckb_resource::{Resource, ResourceLocator};
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use serde_derive::Deserialize;
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

#[derive(Deserialize)]
struct ChainSpecConfig {
    pub name: String,
    pub genesis: Genesis,
    pub params: Params,
    pub system_cells: Vec<SystemCell>,
    pub pow: Pow,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize)]
pub struct Params {
    pub initial_block_reward: Capacity,
    pub max_block_cycles: Cycle,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize)]
pub struct Seal {
    pub nonce: u64,
    pub proof: Vec<u8>,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize)]
pub struct Genesis {
    pub seal: Seal,
    pub version: u32,
    pub parent_hash: H256,
    pub timestamp: u64,
    pub txs_commit: H256,
    pub txs_proposal: H256,
    pub difficulty: U256,
    pub uncles_hash: H256,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize)]
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

    fn build_system_cell_transaction(&self) -> Result<Transaction, Box<Error>> {
        let outputs_result: Result<Vec<_>, _> = self
            .system_cells
            .iter()
            .map(|c| {
                c.get().map(|data| {
                    let data = data.into_owned();
                    // TODO: we should provide a proper lock script here so system cells
                    // can be updated.
                    CellOutput::new(data.len() as u64, data, Script::default(), None)
                })
            })
            .collect();

        let outputs = outputs_result?;

        Ok(TransactionBuilder::default().outputs(outputs).build())
    }

    pub fn to_consensus(&self) -> Result<Consensus, Box<Error>> {
        let header = HeaderBuilder::default()
            .version(self.genesis.version)
            .parent_hash(self.genesis.parent_hash.clone())
            .timestamp(self.genesis.timestamp)
            .txs_commit(self.genesis.txs_commit.clone())
            .txs_proposal(self.genesis.txs_proposal.clone())
            .difficulty(self.genesis.difficulty.clone())
            .nonce(self.genesis.seal.nonce)
            .proof(self.genesis.seal.proof.to_vec())
            .uncles_hash(self.genesis.uncles_hash.clone())
            .build();

        let genesis_block = BlockBuilder::default()
            .commit_transaction(self.build_system_cell_transaction()?)
            .header(header)
            .build();

        let consensus = Consensus::default()
            .set_id(self.name.clone())
            .set_genesis_block(genesis_block)
            .set_initial_block_reward(self.params.initial_block_reward)
            .set_max_block_cycles(self.params.max_block_cycles)
            .set_pow(self.pow.clone());

        Ok(consensus)
    }
}

#[cfg(test)]
pub mod test {
    use super::*;

    #[test]
    fn test_chain_spec_load() {
        let locator = ResourceLocator::current_dir().unwrap();
        let ckb = locator.ckb();
        let dev = ChainSpec::resolve_relative_to(&locator, PathBuf::from("specs/dev.toml"), &ckb);
        assert!(dev.is_ok(), format!("{:?}", dev));
    }

    #[test]
    fn always_success_type_hash() {
        let locator = ResourceLocator::current_dir().unwrap();
        let ckb = locator.ckb();
        let dev = ChainSpec::resolve_relative_to(&locator, PathBuf::from("specs/dev.toml"), &ckb)
            .unwrap();
        let tx = dev.build_system_cell_transaction().unwrap();

        // Tx and Output hash will be used in some test cases directly, assert here for convenience
        assert_eq!(
            format!("{:x}", tx.hash()),
            "9c3c3cc1a11966ff78a739a1ddb5e4b94fdcaa4e63e3e341c6f8126de2dfa2ac"
        );

        let reference = tx.outputs()[0].data_hash();
        assert_eq!(
            format!("{:x}", reference),
            "28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5"
        );

        let script = Script::new(0, vec![], reference);
        assert_eq!(
            format!("{:x}", script.hash()),
            "9a9a6bdbc38d4905eace1822f85237e3a1e238bb3f277aa7b7c8903441123510"
        );
    }

    #[test]
    fn test_testnet_chain_spec_load() {
        let locator = ResourceLocator::current_dir().unwrap();
        let ckb = locator.ckb();
        let testnet =
            ChainSpec::resolve_relative_to(&locator, PathBuf::from("specs/testnet.toml"), &ckb);
        assert!(testnet.is_ok(), format!("{:?}", testnet));
        let chain_spec = testnet.unwrap();

        let result = chain_spec.build_system_cell_transaction();
        assert!(result.is_ok(), format!("{:?}", result));
        let tx = result.unwrap();

        let data_hash = tx.outputs()[0].data_hash();
        assert_eq!(
            format!("{:x}", data_hash),
            "55a809b92c5c404989bfe523639a741f4368ecaa3d4c42d1eb8854445b1b798b"
        );
    }
}
