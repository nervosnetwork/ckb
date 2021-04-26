use crate::{CKBAppConfig, MemoryTrackerConfig, MinerConfig};
use ckb_chain_spec::consensus::Consensus;
use ckb_jsonrpc_types::ScriptHashType;
use ckb_pow::PowEngine;
use ckb_types::packed::Byte32;
use faketime::unix_time_as_millis;
use std::path::PathBuf;
use std::sync::Arc;

/// Parsed command line arguments for `ckb export`.
pub struct ExportArgs {
    /// Parsed `ckb.toml`.
    pub config: Box<CKBAppConfig>,
    /// Loaded consensus.
    pub consensus: Consensus,
    /// The target directory to save the exported file.
    pub target: PathBuf,
}

/// Parsed command line arguments for `ckb import`.
pub struct ImportArgs {
    /// Parsed `ckb.toml`.
    pub config: Box<CKBAppConfig>,
    /// Loaded consensus.
    pub consensus: Consensus,
    /// The path to the file to be imported.
    pub source: PathBuf,
}

/// Parsed command line arguments for `ckb run`.
pub struct RunArgs {
    /// Parsed `ckb.toml`.
    pub config: Box<CKBAppConfig>,
    /// Loaded consensus.
    pub consensus: Consensus,
    /// Whether allow advanced block assembler options.
    pub block_assembler_advanced: bool,
    /// Whether skip chain spec hash check
    pub skip_chain_spec_check: bool,
    /// Whether overwrite the chain spec hash in the database with [`RunArgs::chain_spec_hash`]
    ///
    /// [`RunArgs::chain_spec_hash`]: ./struct.RunArgs.html#structfield.chain_spec_hash
    pub overwrite_chain_spec: bool,
    /// Hash of serialized configured chain spec
    pub chain_spec_hash: Byte32,
}

/// Enable profile on blocks in the range `[from, to]`.
pub type ProfileArgs = Option<(Option<u64>, Option<u64>)>;

/// Parsed command line arguments for `ckb replay`.
pub struct ReplayArgs {
    /// Parsed `ckb.toml`.
    pub config: Box<CKBAppConfig>,
    /// Loaded consensus.
    pub consensus: Consensus,
    /// The directory to store the temporary files during the replay.
    pub tmp_target: PathBuf,
    /// Enable profile on blocks in the range `[from, to]`.
    pub profile: ProfileArgs,
    /// Enable sanity check.
    pub sanity_check: bool,
    /// Enable full verification.
    pub full_verification: bool,
}

/// Parsed command line arguments for `ckb miner`.
pub struct MinerArgs {
    /// Parsed `ckb-miner.toml`.
    pub config: MinerConfig,
    /// Selected PoW algorithm.
    pub pow_engine: Arc<dyn PowEngine>,
    /// Options to configure the memory tracker.
    pub memory_tracker: MemoryTrackerConfig,
    /// The miner process will exit when there are `limit` nounces (puzzle solutions) found. Set it
    /// to 0 to loop forever.
    pub limit: u128,
}

/// Parsed command line arguments for `ckb stats`.
pub struct StatsArgs {
    /// Parsed `ckb.toml`.
    pub config: Box<CKBAppConfig>,
    /// Loaded consensus.
    pub consensus: Consensus,
    /// Specifies the starting block number. The default is 1.
    pub from: Option<u64>,
    /// Specifies the ending block number. The default is the tip block in the database.
    pub to: Option<u64>,
}

/// Parsed command line arguments for `ckb init`.
pub struct InitArgs {
    /// Whether to prompt user inputs interactively.
    pub interactive: bool,
    /// The CKB root directory.
    pub root_dir: PathBuf,
    /// The chain name that this node will join.
    pub chain: String,
    /// RPC port.
    pub rpc_port: String,
    /// P2P port.
    pub p2p_port: String,
    /// Whether to save the logs into the log file.
    pub log_to_file: bool,
    /// Whether to print the logs on the process stdout.
    pub log_to_stdout: bool,
    /// Asks to list available chains.
    pub list_chains: bool,
    /// Force file overwriting.
    pub force: bool,
    /// Block assembler lock script code hash.
    pub block_assembler_code_hash: Option<String>,
    /// Block assembler lock script args.
    pub block_assembler_args: Vec<String>,
    /// Block assembler lock script hash type.
    pub block_assembler_hash_type: ScriptHashType,
    /// Block assembler cellbase transaction message.
    pub block_assembler_message: Option<String>,
    /// Import the spec file.
    ///
    /// When this is set to `-`, the spec file is imported from stdin and the file content must be
    /// encoded by base64. Otherwise it must be a path to the spec file.
    ///
    /// The spec file will be saved into `specs/{CHAIN}.toml`, where `CHAIN` is the chain name.
    pub import_spec: Option<String>,
    /// Customize parameters for chain spec or not.
    ///
    /// Only works for dev chains.
    pub customize_spec: CustomizeSpec,
}

/// Customize parameters for chain spec.
pub struct CustomizeSpec {
    /// Specify a string as the genesis message.
    pub genesis_message: Option<String>,
}

/// Parsed command line arguments for `ckb reset-data`.
pub struct ResetDataArgs {
    /// Reset without asking for user confirmation.
    pub force: bool,
    /// Reset all data.
    pub all: bool,
    /// Reset database.
    pub database: bool,
    /// Reset all network data, including the secret key and peer store.
    pub network: bool,
    /// Reset network peer store.
    pub network_peer_store: bool,
    /// Reset network secret key.
    pub network_secret_key: bool,
    /// Clean logs directory.
    pub logs: bool,
    /// The path to the CKB data directory.
    pub data_dir: PathBuf,
    /// The path to the database directory.
    pub db_path: PathBuf,
    /// The path to the network data directory.
    pub network_dir: PathBuf,
    /// The path to the network peer store directory.
    pub network_peer_store_path: PathBuf,
    /// The path to the network secret key.
    pub network_secret_key_path: PathBuf,
    /// The path to the logs directory.
    pub logs_dir: Option<PathBuf>,
}

/// Parsed command line arguments for `ckb peer-id`.
pub struct PeerIDArgs {
    /// The peer ID read from the secret key file.
    pub peer_id: secio::PeerId,
}

/// Parsed command line arguments for `ckb migrate`.
pub struct MigrateArgs {
    /// The parsed `ckb.toml.`
    pub config: Box<CKBAppConfig>,
    /// Loaded consensus.
    pub consensus: Consensus,
    /// Check whether it is required to do migration instead of really perform the migration.
    pub check: bool,
    /// Do migration without interactive prompt.
    pub force: bool,
}

/// Parsed command line arguments for `ckb db-repair`.
pub struct RepairArgs {
    /// Parsed `ckb.toml`.
    pub config: Box<CKBAppConfig>,
}

impl CustomizeSpec {
    /// No specified parameters for chain spec.
    pub fn is_unset(&self) -> bool {
        self.genesis_message.is_none()
    }

    /// Generates a vector of key-value pairs.
    pub fn key_value_pairs(&self) -> Vec<(&'static str, String)> {
        let mut vec = Vec::new();
        let genesis_message = self
            .genesis_message
            .clone()
            .unwrap_or_else(|| unix_time_as_millis().to_string());
        vec.push(("genesis_message", genesis_message));
        vec
    }
}
