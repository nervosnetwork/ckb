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
use ckb_core::block::Block;
use ckb_core::block::BlockBuilder;
use ckb_core::header::HeaderBuilder;
use ckb_core::script::Script;
use ckb_core::transaction::{CellOutput, Transaction, TransactionBuilder};
use ckb_core::{BlockNumber, Capacity, Cycle};
use ckb_pow::{Pow, PowEngine};
use ckb_resource::{Resource, ResourceLocator};
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
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
    pub initial_block_reward: Capacity,
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
    pub proof: Vec<u8>,
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

    fn build_system_cell_transaction(&self) -> Result<Transaction, Box<Error>> {
        let outputs_result: Result<Vec<_>, _> = self
            .system_cells
            .iter()
            .map(|c| {
                c.get()
                    .map_err(|err| Box::new(err) as Box<Error>)
                    .and_then(|data| {
                        let data = data.into_owned();
                        // TODO: we should provide a proper lock script here so system cells
                        // can be updated.
                        Capacity::bytes(data.len())
                            .map(|cap| CellOutput::new(cap, data, Script::default(), None))
                            .map_err(|err| Box::new(err) as Box<Error>)
                    })
            })
            .collect();

        let outputs = outputs_result?;

        Ok(TransactionBuilder::default().outputs(outputs).build())
    }

    fn verify_genesis_hash(&self, genesis: &Block) -> Result<(), Box<Error>> {
        if let Some(ref expect) = self.genesis.hash {
            let actual = genesis.header().hash();
            if &actual != expect {
                return Err(GenesisError {
                    actual,
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
            .proof(self.genesis.seal.proof.to_vec())
            .uncles_hash(self.genesis.uncles_hash.clone());

        let genesis_block = BlockBuilder::default()
            .transaction(self.build_system_cell_transaction()?)
            .with_header_builder(header_builder);

        self.verify_genesis_hash(&genesis_block)?;

        let consensus = Consensus::default()
            .set_id(self.name.clone())
            .set_genesis_block(genesis_block)
            .set_cellbase_maturity(self.params.cellbase_maturity)
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
        let dev = ChainSpec::resolve_relative_to(&locator, PathBuf::from("specs/dev.toml"), &ckb);
        assert!(dev.is_ok(), format!("{:?}", dev));

        let chain_spec = dev.unwrap();
        let tx = chain_spec.build_system_cell_transaction().unwrap();

        // Tx and Output hash will be used in some test cases directly, assert here for convenience
        assert_eq!(
            format!("{:x}", tx.hash()),
            "48168c5b2460bfa698f60e67f08df5298b1d43b2da7939a219ffd863e1380d11"
        );

        let reference = tx.outputs()[0].data_hash();
        assert_eq!(
            format!("{:x}", reference),
            "28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5"
        );

        let script = Script::new(vec![], reference);
        assert_eq!(
            format!("{:x}", script.hash()),
            "9a9a6bdbc38d4905eace1822f85237e3a1e238bb3f277aa7b7c8903441123510"
        );

        assert!(
            chain_spec.to_consensus().is_ok(),
            format!("{:?}", chain_spec.to_consensus())
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
            "8bddddc3ae2e09c13106634d012525aa32fc47736456dba11514d352845e561d"
        );

        assert!(
            chain_spec.to_consensus().is_ok(),
            format!("{:?}", chain_spec.to_consensus())
        );
    }
}
