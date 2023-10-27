use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use std::path::PathBuf;

/// Indexer sync config options.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexerSyncConfig {
    /// The secondary_db path, default `data_dir / indexer / secondary_path`
    #[serde(default)]
    pub secondary_path: PathBuf,
    /// The poll interval by secs
    #[serde(default = "default_poll_interval")]
    pub poll_interval: u64,
    /// Whether to index the pending txs in the ckb tx-pool
    pub index_tx_pool: bool,
    /// Maximum number of concurrent db background jobs (compactions and flushes)
    #[serde(default)]
    pub db_background_jobs: Option<NonZeroUsize>,
    /// Maximal db info log files to be kept.
    #[serde(default)]
    pub db_keep_log_file_num: Option<NonZeroUsize>,
}

const fn default_poll_interval() -> u64 {
    2
}
