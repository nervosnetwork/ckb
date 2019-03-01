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
use ckb_protocol::Script as FbsScript;
use flatbuffers::FlatBufferBuilder;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use serde_derive::Deserialize;
use std::error::Error;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub mod consensus;

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
    pub cellbase_id: H256,
    pub uncles_hash: H256,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize)]
pub struct SystemCell {
    pub path: PathBuf,
}

fn build_system_cell_transaction(cells: &[SystemCell]) -> Result<Transaction, Box<Error>> {
    let mut outputs = Vec::new();
    for system_cell in cells {
        let mut file = File::open(&system_cell.path)?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;

        let script = Script::new(0, vec![], None, Some(data), vec![]);
        let mut builder = FlatBufferBuilder::new();
        let offset = FbsScript::build(&mut builder, &script);
        builder.finish(offset, None);
        let script_data = builder.finished_data().to_vec();

        // TODO: we should either provide a valid type hash so we can
        // update system cell, or we can update this when P2SH is moved into VM.
        let output = CellOutput::new(
            script_data.len() as Capacity,
            script_data,
            H256::default(),
            None,
        );
        outputs.push(output);
    }

    Ok(TransactionBuilder::default().outputs(outputs).build())
}

impl ChainSpec {
    pub fn read_from_file<P: AsRef<Path>>(path: P) -> Result<ChainSpec, Box<Error>> {
        let config_str = std::fs::read_to_string(path.as_ref())?;
        let mut spec: Self = toml::from_str(&config_str)?;
        spec.resolve_paths(path.as_ref().parent().unwrap());
        Ok(spec)
    }

    pub fn pow_engine(&self) -> Arc<dyn PowEngine> {
        self.pow.engine()
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
            .cellbase_id(self.genesis.cellbase_id.clone())
            .uncles_hash(self.genesis.uncles_hash.clone())
            .build();

        let genesis_block = BlockBuilder::default()
            .commit_transaction(build_system_cell_transaction(&self.system_cells)?)
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
        let dev = ChainSpec::read_from_file(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../nodes_template/spec/dev.toml"),
        );
        assert!(dev.is_ok(), format!("{:?}", dev));
        for cell in &dev.unwrap().system_cells {
            assert!(cell.path.exists());
        }
    }
}
