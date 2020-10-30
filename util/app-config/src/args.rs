use crate::{CKBAppConfig, MemoryTrackerConfig, MinerConfig};
use ckb_chain_spec::consensus::Consensus;
use ckb_jsonrpc_types::ScriptHashType;
use ckb_pow::PowEngine;
use std::path::PathBuf;
use std::sync::Arc;

/// TODO(doc): @doitian
pub struct ExportArgs {
    /// TODO(doc): @doitian
    pub config: Box<CKBAppConfig>,
    /// TODO(doc): @doitian
    pub consensus: Consensus,
    /// TODO(doc): @doitian
    pub target: PathBuf,
}

/// TODO(doc): @doitian
pub struct ImportArgs {
    /// TODO(doc): @doitian
    pub config: Box<CKBAppConfig>,
    /// TODO(doc): @doitian
    pub consensus: Consensus,
    /// TODO(doc): @doitian
    pub source: PathBuf,
}

/// TODO(doc): @doitian
pub struct RunArgs {
    /// TODO(doc): @doitian
    pub config: Box<CKBAppConfig>,
    /// TODO(doc): @doitian
    pub consensus: Consensus,
    /// TODO(doc): @doitian
    pub block_assembler_advanced: bool,
}

pub type ProfileArgs = Option<(Option<u64>, Option<u64>)>;
/// TODO(doc): @doitian
pub struct ReplayArgs {
    /// TODO(doc): @doitian
    pub config: Box<CKBAppConfig>,
    /// TODO(doc): @doitian
    pub consensus: Consensus,
    /// TODO(doc): @doitian
    pub tmp_target: PathBuf,
    /// TODO(doc): @doitian
    pub profile: ProfileArgs,
    /// TODO(doc): @doitian
    pub sanity_check: bool,
    /// TODO(doc): @doitian
    pub full_verfication: bool,
}

/// TODO(doc): @doitian
pub struct MinerArgs {
    /// TODO(doc): @doitian
    pub config: MinerConfig,
    /// TODO(doc): @doitian
    pub pow_engine: Arc<dyn PowEngine>,
    /// TODO(doc): @doitian
    pub memory_tracker: MemoryTrackerConfig,
    /// TODO(doc): @doitian
    pub limit: u128,
}

/// TODO(doc): @doitian
pub struct StatsArgs {
    /// TODO(doc): @doitian
    pub config: Box<CKBAppConfig>,
    /// TODO(doc): @doitian
    pub consensus: Consensus,
    /// TODO(doc): @doitian
    pub from: Option<u64>,
    /// TODO(doc): @doitian
    pub to: Option<u64>,
}

/// TODO(doc): @doitian
pub struct InitArgs {
    /// TODO(doc): @doitian
    pub interactive: bool,
    /// TODO(doc): @doitian
    pub root_dir: PathBuf,
    /// TODO(doc): @doitian
    pub chain: String,
    /// TODO(doc): @doitian
    pub rpc_port: String,
    /// TODO(doc): @doitian
    pub p2p_port: String,
    /// TODO(doc): @doitian
    pub log_to_file: bool,
    /// TODO(doc): @doitian
    pub log_to_stdout: bool,
    /// TODO(doc): @doitian
    pub list_chains: bool,
    /// TODO(doc): @doitian
    pub force: bool,
    /// TODO(doc): @doitian
    pub block_assembler_code_hash: Option<String>,
    /// TODO(doc): @doitian
    pub block_assembler_args: Vec<String>,
    /// TODO(doc): @doitian
    pub block_assembler_hash_type: ScriptHashType,
    /// TODO(doc): @doitian
    pub block_assembler_message: Option<String>,
    /// TODO(doc): @doitian
    pub import_spec: Option<String>,
}

/// TODO(doc): @doitian
pub struct ResetDataArgs {
    /// TODO(doc): @doitian
    pub force: bool,
    /// TODO(doc): @doitian
    pub all: bool,
    /// TODO(doc): @doitian
    pub database: bool,
    /// TODO(doc): @doitian
    pub indexer: bool,
    /// TODO(doc): @doitian
    pub network: bool,
    /// TODO(doc): @doitian
    pub network_peer_store: bool,
    /// TODO(doc): @doitian
    pub network_secret_key: bool,
    /// TODO(doc): @doitian
    pub logs: bool,
    /// TODO(doc): @doitian
    pub data_dir: PathBuf,
    /// TODO(doc): @doitian
    pub db_path: PathBuf,
    /// TODO(doc): @doitian
    pub indexer_db_path: PathBuf,
    /// TODO(doc): @doitian
    pub network_dir: PathBuf,
    /// TODO(doc): @doitian
    pub network_peer_store_path: PathBuf,
    /// TODO(doc): @doitian
    pub network_secret_key_path: PathBuf,
    /// TODO(doc): @doitian
    pub logs_dir: Option<PathBuf>,
}

/// TODO(doc): @doitian
pub struct PeerIDArgs {
    /// TODO(doc): @doitian
    pub peer_id: secio::PeerId,
}

/// TODO(doc): @doitian
pub struct MigrateArgs {
    /// TODO(doc): @doitian
    pub config: Box<CKBAppConfig>,
}
