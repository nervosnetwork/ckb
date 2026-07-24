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
    /// Maximum conservative resident-byte charge of accepted pool entries.
    ///
    /// Unlike `max_tx_pool_size`, this includes resolved input/cell-dep
    /// metadata and eagerly loaded dep-group data. Keeping the limits
    /// separate preserves the public serialized-size semantics while
    /// bounding transactions whose dependency expansion is much larger than
    /// their wire representation.
    pub max_tx_pool_resident_size: usize,
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
    /// Maximum conservative resident-byte charge of the complete pre-pool
    /// pipeline.
    ///
    /// The unified coordinator owns raw, dependency-waiting, resolved,
    /// verifying, conflict-waiting, and commit-ready entries under one global
    /// budget. Charges include expanded resolved-cell data and lifecycle/index
    /// metadata, not only serialized transaction bytes.
    pub max_tx_pipeline_resident_size: usize,
}

impl TxPoolConfig {
    /// Effective accepted-pool residency budget, clamped to the serialized
    /// transaction budget so ordinary pool capacity is never configured
    /// below `max_tx_pool_size` accidentally.
    pub fn tx_pool_resident_size_budget(&self) -> usize {
        self.max_tx_pool_resident_size.max(self.max_tx_pool_size)
    }

    /// Configured pre-pool residency budget. Runtime construction validates
    /// that it can hold at least one conservatively charged entry instead of
    /// silently turning zero into a different configuration.
    pub fn tx_pipeline_resident_size_budget(&self) -> usize {
        self.max_tx_pipeline_resident_size
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
