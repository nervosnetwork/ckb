use ckb_jsonrpc_types::{FeeRateDef, JsonBytes, ScriptHashType};
use ckb_types::core::{Cycle, FeeRate};
use ckb_types::H256;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// The default values are set in the legacy version.
/// Transaction pool configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TxPoolConfig {
    /// Keep the transaction pool below <max_mem_size> mb
    pub max_mem_size: usize,
    /// Keep the transaction pool below <max_cycles> cycles
    pub max_cycles: Cycle,
    /// txs with lower fee rate than this will not be relayed or be mined
    #[serde(with = "FeeRateDef")]
    pub min_fee_rate: FeeRate,
    /// tx pool rejects txs that cycles greater than max_tx_verify_cycles
    pub max_tx_verify_cycles: Cycle,
    /// max ancestors size limit for a single tx
    pub max_ancestors_count: usize,
    /// The file to cache the tx pool state when tx pool have been shutdown.
    ///
    /// By default, it is a file inside the data directory.
    #[serde(default)]
    pub state_file: PathBuf,
}

impl Default for TxPoolConfig {
    fn default() -> Self {
        TxPoolConfig {
            max_mem_size: 20_000_000, // 20mb
            max_cycles: 200_000_000_000,
            min_fee_rate: DEFAULT_MIN_FEE_RATE,
            max_tx_verify_cycles: DEFAULT_MAX_TX_VERIFY_CYCLES,
            max_ancestors_count: DEFAULT_MAX_ANCESTORS_COUNT,
            state_file: Default::default(),
        }
    }
}

/// Block assembler config options.
///
/// The block assembler section tells CKB how to claim the miner rewards.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockAssemblerConfig {
    /// The miner lock script code hash.
    pub code_hash: H256,
    /// The miner lock script args.
    pub args: JsonBytes,
    /// An arbitrary message to be added into the cellbase transaction.
    pub message: JsonBytes,
    /// The miner lock script hash type.
    pub hash_type: ScriptHashType,
    /// Use ckb binary version as message prefix to identify the block miner client (default true, false to disable it).
    #[serde(default = "default_use_binary_version_as_message_prefix")]
    pub use_binary_version_as_message_prefix: bool,
    /// A field to store the block miner client version, non-configurable options.
    #[serde(skip)]
    pub binary_version: String,
}

const fn default_use_binary_version_as_message_prefix() -> bool {
    true
}

impl TxPoolConfig {
    /// Canonicalizes paths in the config options.
    ///
    /// If `self.state_file` is not set, set it to `data_dir / tx_pool.state`.
    ///
    /// If `self.path` is relative, convert them to absolute path using
    /// `root_dir` as current working directory.
    pub fn adjust<P: AsRef<Path>>(&mut self, root_dir: &Path, data_dir: P) {
        if self.state_file.to_str().is_none() || self.state_file.to_str() == Some("") {
            self.state_file = data_dir.as_ref().to_path_buf().join("tx_pool.state");
        } else if self.state_file.is_relative() {
            self.state_file = root_dir.to_path_buf().join(&self.state_file)
        }
    }
}
