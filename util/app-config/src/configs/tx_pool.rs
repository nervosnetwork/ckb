use ckb_jsonrpc_types::{FeeRateDef, JsonBytes, ScriptHashType};
use ckb_types::H256;
use ckb_types::core::{Cycle, FeeRate};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use url::Url;

/// Ordering strategy for the verify queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyOrdering {
    /// FIFO — first-come first-served by arrival time (default).
    ArrivalTime,
    /// Highest fee rate is verified first; ties broken by arrival time.
    FeeRate,
}

/// Default verify queue ordering: FIFO by arrival time.
#[allow(dead_code)]
pub fn default_verify_ordering() -> VerifyOrdering {
    VerifyOrdering::ArrivalTime
}

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
    /// max tx verify workers, default is 3/4 of cpu cores
    #[serde(default = "default_max_tx_verify_workers")]
    pub max_tx_verify_workers: usize,
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
    /// Verify queue ordering strategy: arrival_time (FIFO) or fee_rate.
    #[serde(default = "default_verify_ordering")]
    pub verify_ordering: VerifyOrdering,
    /// Max total serialized size (in bytes) of transactions queued in the
    /// verify queue.
    ///
    /// The verify queue is the slowest tx-pool pipeline stage (VM execution)
    /// and the one whose entries carry the most completed work, so it gets a
    /// larger budget than the other pipeline queues. The effective budget is
    /// clamped up to `max_tx_pool_size` (see
    /// [`TxPoolConfig::verify_queue_tx_size_budget`]): the pool itself may
    /// hold that much, and bursts plus reload churn should not squeeze the
    /// queue below it. (Reload itself goes through the direct sync path and
    /// does not pass through this queue; the clamp is headroom, not a
    /// reload requirement.) Note the queue is "full" at
    /// `total + add >= budget`, i.e. the budget minus one byte effectively.
    pub max_verify_queue_tx_size: usize,
}

impl TxPoolConfig {
    /// Effective verify-queue budget in bytes: `max_verify_queue_tx_size`
    /// clamped up to `max_tx_pool_size` (headroom so bursts and reload
    /// churn cannot squeeze the queue below the pool's own capacity).
    pub fn verify_queue_tx_size_budget(&self) -> usize {
        self.max_verify_queue_tx_size.max(self.max_tx_pool_size)
    }
}

/// default max tx verify workers is 3/4 of cpu cores
pub fn default_max_tx_verify_workers() -> usize {
    std::cmp::max(num_cpus::get() * 3 / 4, 1)
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
