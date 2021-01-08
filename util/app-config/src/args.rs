use crate::{CKBAppConfig, MemoryTrackerConfig, MinerConfig};
use ckb_chain_spec::consensus::Consensus;
use ckb_jsonrpc_types::ScriptHashType;
use ckb_pow::PowEngine;
use ckb_types::packed::Byte32;
use faketime::unix_time_as_millis;
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
    /// Whether skip chain spec hash check
    pub skip_chain_spec_check: bool,
    /// Config chain spec hash
    pub chain_spec_hash: Byte32,
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
    pub full_verification: bool,
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
    /// Don't customize any parameters for chain spec, use the default parameters.
    ///
    /// Only works for dev chains.
    pub use_default_spec: bool,
    /// Customize parameters for chain spec or not.
    ///
    /// Only works for dev chains.
    pub customize_spec: CustomizeSpec,
}

/// Customize parameters for chain spec.
pub struct CustomizeSpec {
    /// Specify a timestamp as the genesis timestamp.
    /// If no timestamp is provided, use current timestamp.
    pub genesis_timestamp: Option<u64>,
    /// Specify a string as the genesis message.
    pub genesis_message: Option<String>,
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
    /// check flag present
    pub check: bool,
}

impl CustomizeSpec {
    /// No specified parameters for chain spec.
    pub fn is_unset(&self) -> bool {
        self.genesis_timestamp.is_none() && self.genesis_message.is_none()
    }

    /// Generates a vector of key-value pairs.
    pub fn key_value_pairs(&self) -> Vec<(&'static str, String)> {
        let mut vec = Vec::new();
        let genesis_timestamp = self
            .genesis_timestamp
            .unwrap_or_else(unix_time_as_millis)
            .to_string();
        let genesis_message = self.genesis_message.clone().unwrap_or_else(String::new);
        vec.push(("genesis_timestamp", genesis_timestamp));
        vec.push(("genesis_message", genesis_message));
        vec
    }
}
