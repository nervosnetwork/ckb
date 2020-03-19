use super::app_config::CKBAppConfig;
use ckb_chain_spec::consensus::Consensus;
use ckb_jsonrpc_types::ScriptHashType;
use ckb_memory_tracker::Config as MemoryTrackerConfig;
use ckb_miner::MinerConfig;
use ckb_pow::PowEngine;
use std::path::PathBuf;
use std::sync::Arc;

pub struct ExportArgs {
    pub config: Box<CKBAppConfig>,
    pub consensus: Consensus,
    pub target: PathBuf,
}

pub struct ImportArgs {
    pub config: Box<CKBAppConfig>,
    pub consensus: Consensus,
    pub source: PathBuf,
}

pub struct RunArgs {
    pub config: Box<CKBAppConfig>,
    pub consensus: Consensus,
    pub block_assembler_advanced: bool,
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
    pub memory_tracker: MemoryTrackerConfig,
}

pub struct StatsArgs {
    pub config: Box<CKBAppConfig>,
    pub consensus: Consensus,
    pub from: Option<u64>,
    pub to: Option<u64>,
}

pub struct InitArgs {
    pub interactive: bool,
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
    pub block_assembler_hash_type: ScriptHashType,
    pub block_assembler_message: Option<String>,
    pub import_spec: Option<String>,
}

pub struct ResetDataArgs {
    pub force: bool,
    pub all: bool,
    pub database: bool,
    pub indexer: bool,
    pub network: bool,
    pub network_peer_store: bool,
    pub network_secret_key: bool,
    pub logs: bool,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub indexer_db_path: PathBuf,
    pub network_dir: PathBuf,
    pub network_peer_store_path: PathBuf,
    pub network_secret_key_path: PathBuf,
    pub logs_dir: Option<PathBuf>,
}
