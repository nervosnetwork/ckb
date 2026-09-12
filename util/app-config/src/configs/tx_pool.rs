use ckb_jsonrpc_types::{FeeRateDef, JsonBytes, ScriptHashType};
use ckb_types::H256;
use ckb_types::core::{Cycle, FeeRate};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use url::Url;

/// Selection order for queued verification; completion may occur out of order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyOrdering {
    /// First-come first-served by arrival time.
    ArrivalTime,
    /// Highest fee rate first, with arrival time breaking ties (default).
    #[default]
    FeeRate,
}

/// Transaction-pool admission, verification and retention policy.
/// Defaults and configuration-file compatibility are defined in the legacy parser.
#[derive(Clone, Debug, Serialize)]
pub struct TxPoolConfig {
    /// Maximum serialized transaction bytes retained in the accepted pool.
    /// The pool derives additional memory and work bounds internally.
    pub max_tx_pool_size: usize,
    /// Minimum fee rate for relay and mining, in shannons per kilobyte.
    #[serde(with = "FeeRateDef")]
    pub min_fee_rate: FeeRate,
    /// Replacement fee rate, in shannons per kilobyte; values above
    /// `min_fee_rate` enable replacement by fee.
    #[serde(with = "FeeRateDef")]
    pub min_rbf_rate: FeeRate,
    /// Maximum ancestor count, including the transaction itself. Configuration
    /// files retain the historical floor of 1,000.
    pub max_ancestors_count: usize,

    /// Selection order for queued verification, defaulting to fee rate.
    pub verify_ordering: VerifyOrdering,
    /// Remote declared-cycle threshold separating small and large work.
    /// The historical name remains accepted, but this is a scheduling policy,
    /// not an admission or VM execution limit.
    pub max_tx_verify_cycles: Cycle,
    /// Verification worker population, defaulting to at least one and otherwise
    /// three quarters of CPU cores. Runtime capacity can further limit concurrent
    /// computation to leave room for control and I/O.
    #[serde(default = "default_max_tx_verify_workers")]
    pub max_tx_verify_workers: usize,
    /// Minimum cumulative active VM-work budget per pool attempt, in milliseconds.
    /// ELF loading counts; queueing, suspension and non-script checks do not.
    pub min_tx_verify_time_ms: u32,
    /// Cycles per millisecond used to select a local VM-work budget between
    /// `min_tx_verify_time_ms` and `max_tx_verify_time_ms`; not consensus accounting.
    pub tx_verify_cycles_per_ms: u64,
    /// Maximum cumulative active VM-work budget per attempt, in milliseconds.
    /// Defaults to one minimum block interval (8 seconds).
    pub max_tx_verify_time_ms: u32,
    /// Maximum cumulative bytes mapped while loading one root program.
    /// This separate loading bound does not exclude loading time from the VM budget.
    pub max_tx_verify_initial_load_bytes: u64,

    /// Transaction expiration time in hours.
    pub expiry_hours: u8,
    /// Retention of recent rejection records in days.
    pub keep_rejected_tx_hashes_days: u8,
    /// Maximum number of recent rejection records.
    pub keep_rejected_tx_hashes_count: u64,
    /// Persistence file base path, defaulting to `data_dir/tx-pool/persisted_data`.
    /// Relative paths are resolved against the node root directory.
    #[serde(default)]
    pub persisted_data: PathBuf,
    /// Recent rejection database directory, defaulting to
    /// `data_dir/tx-pool/recent_reject`. Relative paths use the node root directory.
    #[serde(default)]
    pub recent_reject: PathBuf,
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
    /// Optional bearer token to authenticate block-template notifications.
    ///
    /// When `notify` URLs are configured and this token is set, the node will
    /// send the header `Authorization: Bearer <token>` with every notify
    /// request. The receiving ckb-miner must be configured with the same token
    /// in `miner.client.auth_token`, otherwise notifications will be rejected.
    ///
    /// Must be non-empty and free of leading/trailing whitespace; the node
    /// refuses to start otherwise.
    #[serde(default)]
    pub notify_auth_token: Option<String>,
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
