use ckb_jsonrpc_types::{FeeRateDef, JsonBytes, ScriptHashType};
use ckb_types::core::{Cycle, FeeRate};
use ckb_types::H256;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use url::Url;

// The default values are set in the legacy version.
/// Transaction pool configuration
#[derive(Clone, Debug, Serialize)]
pub struct TxPoolConfig {
    /// Keep the transaction pool below <max_tx_pool_size> mb
    pub max_tx_pool_size: usize,
    /// txs with lower fee rate than this will not be relayed or be mined
    #[serde(with = "FeeRateDef")]
    pub min_fee_rate: FeeRate,
    /// txs need to pay larger fee rate than this for RBF
    #[serde(with = "FeeRateDef")]
    pub min_rbf_rate: FeeRate,
    /// tx pool rejects txs that cycles greater than max_tx_verify_cycles
    pub max_tx_verify_cycles: Cycle,
    /// max ancestors size limit for a single tx
    pub max_ancestors_count: usize,
    /// rejected tx time to live by days
    pub keep_rejected_tx_hashes_days: u8,
    /// rejected tx count limit
    pub keep_rejected_tx_hashes_count: u64,
    /// The file to persist the tx pool on the disk when tx pool have been shutdown.
    ///
    /// By default, it is a subdirectory of 'tx-pool' subdirectory under the data directory.
    #[serde(default)]
    pub persisted_data: PathBuf,
    /// The recent reject record database directory path.
    ///
    /// By default, it is a subdirectory of 'tx-pool' subdirectory under the data directory.
    #[serde(default)]
    pub recent_reject: PathBuf,
    /// The expiration time for pool transactions in hours
    pub expiry_hours: u8,
}

/// Block assembler config options.
///
/// The block assembler section tells CKB how to claim the miner rewards.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Eq)]
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
    /// A field to control update interval millis
    #[serde(default = "default_update_interval_millis")]
    pub update_interval_millis: u64,
    /// Notify url
    #[serde(default)]
    pub notify: Vec<Url>,
    /// Notify scripts
    #[serde(default)]
    pub notify_scripts: Vec<String>,
    /// Notify timeout
    #[serde(default = "default_notify_timeout_millis")]
    pub notify_timeout_millis: u64,
}

const fn default_use_binary_version_as_message_prefix() -> bool {
    true
}

const fn default_update_interval_millis() -> u64 {
    800
}

const fn default_notify_timeout_millis() -> u64 {
    800
}

impl TxPoolConfig {
    /// Canonicalizes paths in the config options.
    ///
    /// If `self.persisted_data` is not set, set it to `data_dir / tx_pool_persisted_data`.
    ///
    /// If `self.path` is relative, convert them to absolute path using
    /// `root_dir` as current working directory.
    pub fn adjust<P: AsRef<Path>>(&mut self, root_dir: &Path, tx_pool_dir: P) {
        _adjust(
            root_dir,
            tx_pool_dir.as_ref(),
            &mut self.persisted_data,
            "persisted_data",
        );
        _adjust(
            root_dir,
            tx_pool_dir.as_ref(),
            &mut self.recent_reject,
            "recent_reject",
        );
    }
}

fn _adjust(root_dir: &Path, tx_pool_dir: &Path, target: &mut PathBuf, sub: &str) {
    if target.to_str().is_none() || target.to_str() == Some("") {
        *target = tx_pool_dir.to_path_buf().join(sub);
    } else if target.is_relative() {
        *target = root_dir.to_path_buf().join(&target)
    }
}
