//! # The Chain Specification
//!
//! By default, when simply running CKB, CKB will connect to the official public Nervos network.
//!
//! In order to run a chain different to the official public one, CKB provide the --chain option or
//! with a config file specifying chain = "path" under [ckb].
//! There are a few named presets that can be selected from or a custom yaml spec file can be supplied.

extern crate bigint;
extern crate ckb_chain as chain;
extern crate ckb_core as core;
extern crate serde_yaml;
#[macro_use]
extern crate serde_derive;

use bigint::{H256, U256};
use chain::consensus::{Consensus, GenesisBuilder};
use core::transaction::{CellOutput, IndexedTransaction, Transaction};
use core::Capacity;
use std::error::Error;
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
pub enum SpecType {
    Dev,
    Custom(String),
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize)]
pub struct ChainSpec {
    pub name: String,
    pub genesis: Genesis,
    pub params: Params,
    pub system_cells: Vec<SystemCell>,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize)]
pub struct Params {
    pub initial_block_reward: Capacity,
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
    pub cellbase_id: H256,
    pub uncles_hash: H256,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize)]
pub struct SystemCell {
    pub path: String,
}

fn build_system_cell_transaction(cells: &[SystemCell]) -> Result<IndexedTransaction, Box<Error>> {
    let mut outputs = Vec::new();
    for system_cell in cells {
        let mut file = File::open(&system_cell.path)?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;

        // TODO: we should either provide a valid redeem script hash so we can
        // update system cell, or we can update this when P2SH is moved into VM.
        let output = CellOutput::new(data.len() as Capacity, data, H256::default());
        outputs.push(output);
    }

    let transaction = Transaction::new(0, Vec::new(), Vec::new(), outputs);
    let hash = transaction.hash();
    Ok(IndexedTransaction::new(transaction, hash))
}

impl ChainSpec {
    pub fn read_from_file<P: AsRef<Path>>(path: P) -> Result<ChainSpec, Box<Error>> {
        let file = File::open(path)?;
        let spec = serde_yaml::from_reader(file)?;
        Ok(spec)
    }

    pub fn new_dev() -> Result<ChainSpec, Box<Error>> {
        let mut spec: ChainSpec = serde_yaml::from_str(include_str!("../res/dev.yaml"))?;
        let system_cell_path = Path::new(file!()).parent().unwrap().join("../res/cells");
        for cell in &mut spec.system_cells {
            let path = system_cell_path.join(&cell.path);
            let path_str = path.to_str().ok_or("invalid cell path")?;
            cell.path = path_str.to_string();
        }
        Ok(spec)
    }

    pub fn to_consensus(&self) -> Result<Consensus, Box<Error>> {
        let genesis_block = GenesisBuilder::new()
            .version(self.genesis.version)
            .parent_hash(self.genesis.parent_hash)
            .timestamp(self.genesis.timestamp)
            .txs_commit(self.genesis.txs_commit)
            .txs_proposal(self.genesis.txs_proposal)
            .difficulty(self.genesis.difficulty)
            .seal(self.genesis.seal.nonce, self.genesis.seal.proof.clone())
            .cellbase_id(self.genesis.cellbase_id)
            .uncles_hash(self.genesis.uncles_hash)
            .add_commit_transaction(build_system_cell_transaction(&self.system_cells)?)
            .build();

        let consensus = Consensus::default()
            .set_genesis_block(genesis_block)
            .set_initial_block_reward(self.params.initial_block_reward);

        Ok(consensus)
    }
}

impl ::std::str::FromStr for SpecType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let spec_type = match s {
            "dev" => SpecType::Dev,
            other => SpecType::Custom(other.into()),
        };
        Ok(spec_type)
    }
}

impl SpecType {
    pub fn load_spec(self) -> Result<ChainSpec, Box<Error>> {
        match self {
            SpecType::Dev => ChainSpec::new_dev(),
            SpecType::Custom(ref filename) => ChainSpec::read_from_file(filename),
        }
    }
}

#[cfg(test)]
pub mod test {
    use super::*;

    #[test]
    fn test_spec_type_parse() {
        assert_eq!(SpecType::Dev, "dev".parse().unwrap());
    }

    #[test]
    fn test_chain_spec_load() {
        let dev = ChainSpec::new_dev();
        assert!(dev.is_ok());
    }
}
