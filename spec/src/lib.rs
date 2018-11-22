//! # The Chain Specification
//!
//! By default, when simply running CKB, CKB will connect to the official public Nervos network.
//!
//! In order to run a chain different to the official public one, CKB provide the --chain option or
//! with a config file specifying chain = "path" under [ckb].
//! There are a few named presets that can be selected from or a custom yaml spec file can be supplied.

extern crate bigint;
extern crate ckb_core;
extern crate dir;
extern crate serde_json;
#[macro_use]
extern crate serde_derive;
extern crate ckb_pow;
#[cfg(test)]
extern crate tempfile;

use bigint::{H256, U256};
use ckb_core::block::BlockBuilder;
use ckb_core::header::HeaderBuilder;
use ckb_core::transaction::{CellOutput, Transaction, TransactionBuilder};
use ckb_core::Capacity;
use ckb_pow::{Pow, PowEngine};
use consensus::Consensus;
use dir::resolve_path_with_relative_dirs;
use std::error::Error;
use std::fs::File;
use std::io::{Error as IOError, ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub mod consensus;

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
    pub pow: Pow,
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

fn build_system_cell_transaction(cells: &[SystemCell]) -> Result<Transaction, Box<Error>> {
    let mut outputs = Vec::new();
    for system_cell in cells {
        let mut file = File::open(&system_cell.path)?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;

        // TODO: we should either provide a valid type hash so we can
        // update system cell, or we can update this when P2SH is moved into VM.
        let output = CellOutput::new(data.len() as Capacity, data, H256::default(), None);
        outputs.push(output);
    }

    Ok(TransactionBuilder::default().outputs(outputs).build())
}

impl ChainSpec {
    pub fn read_from_file<P: AsRef<Path>>(
        path: P,
        relative_dirs: &[PathBuf],
    ) -> Result<ChainSpec, Box<Error>> {
        if let Some(path) = resolve_path_with_relative_dirs(&path, relative_dirs) {
            let file = File::open(&path)?;
            let mut spec: ChainSpec = serde_json::from_reader(file)?;
            let mut dirs = vec![path.parent().unwrap().to_path_buf()];
            dirs.extend_from_slice(relative_dirs);
            spec.update_system_cell_paths(&dirs);
            return Ok(spec);
        }
        Err(Box::new(IOError::new(
            ErrorKind::NotFound,
            "spec not found!",
        )))
    }

    pub fn new_dev() -> Result<ChainSpec, Box<Error>> {
        let bundled_spec_path = Path::new(file!()).parent().unwrap().join("../res/dev.json");
        Self::read_from_file(bundled_spec_path, &[])
    }

    pub fn pow_engine(&self) -> Arc<dyn PowEngine> {
        self.pow.engine()
    }

    pub fn to_consensus(&self) -> Result<Consensus, Box<Error>> {
        let header = HeaderBuilder::default()
            .version(self.genesis.version)
            .parent_hash(&self.genesis.parent_hash)
            .timestamp(self.genesis.timestamp)
            .txs_commit(&self.genesis.txs_commit)
            .txs_proposal(&self.genesis.txs_proposal)
            .difficulty(&self.genesis.difficulty)
            .nonce(self.genesis.seal.nonce)
            .proof(&self.genesis.seal.proof)
            .cellbase_id(&self.genesis.cellbase_id)
            .uncles_hash(&self.genesis.uncles_hash)
            .build();

        let genesis_block = BlockBuilder::default()
            .commit_transaction(build_system_cell_transaction(&self.system_cells)?)
            .header(header)
            .build();

        let consensus = Consensus::default()
            .set_id(self.name.clone())
            .set_genesis_block(genesis_block)
            .set_initial_block_reward(self.params.initial_block_reward)
            .set_pow(self.pow.clone());

        Ok(consensus)
    }

    fn update_system_cell_paths(&mut self, relative_dirs: &[PathBuf]) {
        for cell in &mut self.system_cells {
            if let Some(path) = resolve_path_with_relative_dirs(&cell.path, relative_dirs) {
                cell.path = path.to_str().unwrap().to_string();
            }
        }
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
    pub fn load_spec<P: AsRef<Path>>(
        self,
        relative_dirs: &[PathBuf],
    ) -> Result<ChainSpec, Box<Error>> {
        match self {
            SpecType::Dev => ChainSpec::new_dev(),
            SpecType::Custom(ref filename) => ChainSpec::read_from_file(filename, relative_dirs),
        }
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::path::Path;
    use tempfile;

    #[test]
    fn test_spec_type_parse() {
        assert_eq!(SpecType::Dev, "dev".parse().unwrap());
    }

    #[test]
    fn test_chain_spec_load() {
        let dev = ChainSpec::new_dev();
        assert!(dev.is_ok());
    }

    fn write_file<P: AsRef<Path>>(file: P, content: &str) {
        let mut file = File::create(file).expect("test dir clean");
        file.write_all(content.as_bytes())
            .expect("write test content");;
    }

    #[test]
    fn test_chain_spec_load_adjust_system_cell_paths() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_chain_spec_load_adjust_system_cell_paths")
            .tempdir()
            .unwrap();

        let test_spec = r#"
        {
            "name": "ckb_test_chain_spec",
            "genesis": {
                "seal": {
                    "nonce": 233,
                    "proof": [2, 3, 3]
                },
                "version": 0,
                "parent_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "timestamp": 0,
                "txs_commit": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "txs_proposal": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "difficulty": "0x233",
                "cellbase_id": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
            },
            "params": {
                "initial_block_reward": 233
            },
            "system_cells": [
                {"path": "cell1"},
                {"path": "cell2"}
            ],
            "pow": {
                "Cuckoo": {
                    "edge_bits": 29,
                    "cycle_length": 42
                }
            }
        }
        "#;
        let chain_spec_path = tmp_dir.path().join("ckb_test_custom.json");
        write_file(&chain_spec_path, test_spec);
        let cell1_path = tmp_dir.path().join("cell1");
        write_file(&cell1_path, "cell1");
        let cell2_path = tmp_dir.path().join("cell2");
        write_file(&cell2_path, "cell2");

        let result = ChainSpec::read_from_file(chain_spec_path, &vec![]);
        assert!(result.is_ok());

        let spec = result.unwrap();
        assert_eq!(
            spec.system_cells[0].path,
            cell1_path.to_str().unwrap().to_string()
        );
        assert_eq!(
            spec.system_cells[1].path,
            cell2_path.to_str().unwrap().to_string()
        );
    }

    #[test]
    fn test_chain_spec_load_from_relative_dirs() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_chain_spec_load_from_relative_dirs")
            .tempdir()
            .unwrap();

        let test_spec = r#"
        {
            "name": "ckb_test_chain_spec",
            "genesis": {
                "seal": {
                    "nonce": 233,
                    "proof": [2, 3, 3]
                },
                "version": 0,
                "parent_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "timestamp": 0,
                "txs_commit": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "txs_proposal": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "difficulty": "0x233",
                "cellbase_id": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
            },
            "params": {
                "initial_block_reward": 233
            },
            "system_cells": [],
            "pow": {
                "Cuckoo": {
                    "edge_bits": 29,
                    "cycle_length": 42
                }
            }
        }
        "#;
        let chain_spec_path = tmp_dir.path().join("ckb_test_custom.json");
        write_file(&chain_spec_path, test_spec);

        let result =
            ChainSpec::read_from_file("ckb_test_custom.json", &vec![tmp_dir.path().to_path_buf()]);
        assert!(result.is_ok());
    }
}
