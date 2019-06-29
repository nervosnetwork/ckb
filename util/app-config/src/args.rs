use super::app_config::CKBAppConfig;
use ckb_chain_spec::consensus::Consensus;
use ckb_instrument::Format;
use ckb_miner::MinerConfig;
use ckb_pow::PowEngine;
use std::path::PathBuf;
use std::sync::Arc;

pub struct ExportArgs {
    pub config: Box<CKBAppConfig>,
    pub consensus: Consensus,
    pub format: Format,
    pub target: PathBuf,
}

pub struct ImportArgs {
    pub config: Box<CKBAppConfig>,
    pub consensus: Consensus,
    pub format: Format,
    pub source: PathBuf,
}

pub struct RunArgs {
    pub config: Box<CKBAppConfig>,
    pub consensus: Consensus,
}

pub struct ProfArgs {
    pub config: Box<CKBAppConfig>,
    pub consensus: Consensus,
    pub from: u64,
    pub to: u64,
}

pub struct MinerArgs {
    pub config: MinerConfig,
    pub pow_engine: Arc<dyn PowEngine>,
}

pub struct StatsArgs {
    pub config: Box<CKBAppConfig>,
    pub consensus: Consensus,
    pub from: Option<u64>,
    pub to: Option<u64>,
}

pub struct InitArgs {
    pub root_dir: PathBuf,
    pub chain: String,
    pub rpc_port: String,
    pub p2p_port: String,
    pub log_to_file: bool,
    pub log_to_stdout: bool,
    pub list_chains: bool,
    pub force: bool,
    pub block_assembler_code_hash: Option<String>,
    pub block_assembler_args: Vec<String>,
    pub block_assembler_data: Option<String>,
}
