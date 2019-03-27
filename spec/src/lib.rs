//! # The Chain Specification
//!
//! By default, when simply running CKB, CKB will connect to the official public Nervos network.
//!
//! In order to run a chain different to the official public one,
//! with a config file specifying chain = "path" under [ckb].

// Shields clippy errors in generated chainspecs.rs file.
#![allow(clippy::unreadable_literal)]

use crate::consensus::Consensus;
use ckb_core::block::BlockBuilder;
use ckb_core::header::HeaderBuilder;
use ckb_core::script::Script;
use ckb_core::transaction::{CellOutput, Transaction, TransactionBuilder};
use ckb_core::{Capacity, Cycle};
use ckb_pow::{Pow, PowEngine};
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use serde_derive::Deserialize;
use std::error::Error;
use std::fs::File;
use std::io::Read;
use std::path::{Display, Path, PathBuf};
use std::sync::Arc;

pub mod consensus;

include!(concat!(env!("OUT_DIR"), "/chainspecs.rs"));

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub enum SpecPath {
    Testnet,
    Local(PathBuf),
}

impl SpecPath {
    pub fn display(&self) -> Display {
        match self {
            SpecPath::Testnet => Path::new("Testnet").display(),
            SpecPath::Local(path) => path.display(),
        }
    }

    pub fn expand_path<P: AsRef<Path>>(&self, base: P) -> Self {
        match self {
            SpecPath::Testnet => SpecPath::Testnet,
            SpecPath::Local(path) => {
                if path.is_relative() {
                    SpecPath::Local(base.as_ref().join(path))
                } else {
                    SpecPath::Local(path.to_path_buf())
                }
            }
        }
    }

    fn path(&self) -> PathBuf {
        match self {
            SpecPath::Testnet => PathBuf::from("testnet/testnet.toml"),
            SpecPath::Local(path) => PathBuf::from(path),
        }
    }

    fn load_file<P: AsRef<Path>>(&self, path: P) -> Result<Vec<u8>, Box<Error>> {
        match self {
            SpecPath::Testnet => {
                let s = path.as_ref().to_str().expect("chain spec path");
                Ok(FILES
                    .get(&format!("chainspecs/{}", s))
                    .expect("hardcoded spec")
                    .to_vec())
            }
            SpecPath::Local(_) => {
                let mut file = File::open(&path)?;
                let mut data = Vec::new();
                file.read_to_end(&mut data)?;
                Ok(data)
            }
        }
    }
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

pub(self) fn build_system_cell_transaction(
    cells: &[SystemCell],
    spec_path: &SpecPath,
) -> Result<Transaction, Box<Error>> {
    let mut outputs = Vec::new();
    for system_cell in cells {
        let data = spec_path.load_file(&system_cell.path)?;

        // TODO: we should provide a proper lock script here so system cells
        // can be updated.
        let output = CellOutput::new(data.len() as Capacity, data, Script::default(), None);
        outputs.push(output);
    }

    Ok(TransactionBuilder::default().outputs(outputs).build())
}

impl ChainSpec {
    pub fn read_from_file(spec_path: &SpecPath) -> Result<ChainSpec, Box<Error>> {
        let config_bytes = spec_path.load_file(spec_path.path())?;
        let config_str = String::from_utf8(config_bytes)?;
        let mut spec: Self = toml::from_str(&config_str)?;
        spec.resolve_paths(spec_path.path().parent().expect("chain spec path resolve"));

        Ok(spec)
    }

    pub fn pow_engine(&self) -> Arc<dyn PowEngine> {
        self.pow.engine()
    }

    pub fn to_consensus(&self, spec_path: &SpecPath) -> Result<Consensus, Box<Error>> {
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
            .commit_transaction(build_system_cell_transaction(
                &self.system_cells,
                &spec_path,
            )?)
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

    fn resolve_paths(&mut self, base: &Path) {
        for mut cell in &mut self.system_cells {
            if cell.path.is_relative() {
                cell.path = base.join(&cell.path);
            }
        }
    }
}

#[cfg(test)]
pub mod test {
    use super::*;

    #[test]
    fn test_chain_spec_load() {
        println!(
            "{:?}",
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../nodes_template/spec/dev.toml")
                .display()
        );
        let dev = ChainSpec::read_from_file(&SpecPath::Local(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../nodes_template/spec/dev.toml"),
        ));
        assert!(dev.is_ok(), format!("{:?}", dev));
        for cell in &dev.unwrap().system_cells {
            assert!(cell.path.exists());
        }
    }

    #[test]
    fn always_success_type_hash() {
        let spec_path = SpecPath::Local(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../nodes_template/spec/dev.toml"),
        );
        let always_success_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../nodes_template/spec/cells/always_success");

        let tx = build_system_cell_transaction(
            &[SystemCell {
                path: always_success_path,
            }],
            &spec_path,
        )
        .unwrap();

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
        let spec_path = SpecPath::Testnet;
        let result = ChainSpec::read_from_file(&spec_path);
        assert!(result.is_ok(), format!("{:?}", result));
        let chain_spec = result.unwrap();

        let result = build_system_cell_transaction(&chain_spec.system_cells, &spec_path);
        assert!(result.is_ok(), format!("{:?}", result));
        let tx = result.unwrap();

        let data_hash = tx.outputs()[0].data_hash();
        assert_eq!(
            format!("{:x}", data_hash),
            "fe1cf5a297023a3c5282ecd9b0ca88d6736424d75fbe4dcf47a7c8b303e4d339"
        );
    }
}
