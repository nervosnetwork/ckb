use super::rich_indexer::RichIndexerConfig;

use ckb_types::H256;
use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

/// Indexer config options.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexerConfig {
    /// The index store path, default `data_dir / indexer / store`
    #[serde(default)]
    pub store: PathBuf,
    /// The secondary_db path, default `data_dir / indexer / secondary_path`
    #[serde(default)]
    pub secondary_path: PathBuf,
    /// The poll interval by secs
    #[serde(default = "default_poll_interval")]
    pub poll_interval: u64,
    /// Whether to index the pending txs in the ckb tx-pool
    #[serde(default)]
    pub index_tx_pool: bool,
    /// Customize block filter
    #[serde(default)]
    pub block_filter: Option<String>,
    /// Customize cell filter
    #[serde(default)]
    pub cell_filter: Option<String>,
    /// Maximum number of concurrent db background jobs (compactions and flushes)
    #[serde(default)]
    pub db_background_jobs: Option<NonZeroUsize>,
    /// Maximal db info log files to be kept.
    #[serde(default)]
    pub db_keep_log_file_num: Option<NonZeroUsize>,
    /// The init tip block hash
    #[serde(default)]
    pub init_tip_hash: Option<H256>,
    /// limit of indexer request
    #[serde(default = "default_request_limit")]
    pub request_limit: Option<usize>,
    /// limit of indexer request timeout
    #[serde(default = "default_timeout_limit")]
    pub timeout_limit: Option<u64>,
    /// Rich indexer config options
    #[serde(default)]
    pub rich_indexer: RichIndexerConfig,
}

/// The default cap on how many items one indexer request may return.
///
/// This is the value `ckb.toml` has been recommending for years: serializing the response to
/// JSON costs roughly ten times the size of the raw cell data, so a small page keeps a single
/// request far away from exhausting memory. Operators that really need bigger pages can raise
/// it explicitly; the previous default of "no limit" made every enabled indexer endpoint
/// unbounded by accident rather than by choice.
pub const DEFAULT_REQUEST_LIMIT: usize = 400;

/// The default time budget of one indexer request, in seconds.
pub const DEFAULT_TIMEOUT_LIMIT: u64 = 10;

/// Default for [`IndexerConfig::request_limit`].
fn default_request_limit() -> Option<usize> {
    Some(DEFAULT_REQUEST_LIMIT)
}

/// Default for [`IndexerConfig::timeout_limit`].
fn default_timeout_limit() -> Option<u64> {
    Some(DEFAULT_TIMEOUT_LIMIT)
}

const fn default_poll_interval() -> u64 {
    2
}

impl Default for IndexerConfig {
    fn default() -> Self {
        IndexerConfig {
            poll_interval: 2,
            index_tx_pool: false,
            store: PathBuf::new(),
            secondary_path: PathBuf::new(),
            block_filter: None,
            cell_filter: None,
            db_background_jobs: None,
            db_keep_log_file_num: None,
            init_tip_hash: None,
            request_limit: default_request_limit(),
            timeout_limit: default_timeout_limit(),
            rich_indexer: RichIndexerConfig::default(),
        }
    }
}

impl IndexerConfig {
    /// Canonicalizes paths in the config options.
    ///
    /// If `self.store` is not set, set it to `data_dir / indexer / store`.
    ///
    /// If `self.secondary_path` is not set, set it to `data_dir / indexer / secondary_path`.
    ///
    /// If `self.rich_indexer` is `Sqlite`, and `self.rich_indexer.sqlite.store` is not set,
    /// set it to `data_dir / indexer / sqlite / sqlite.db`.
    ///
    /// If any of the above paths is relative, convert them to absolute path using
    /// `root_dir` as current working directory.
    pub fn adjust<P: AsRef<Path>>(&mut self, root_dir: &Path, indexer_dir: P) {
        _adjust(root_dir, indexer_dir.as_ref(), &mut self.store, "store");
        _adjust(
            root_dir,
            indexer_dir.as_ref(),
            &mut self.secondary_path,
            "secondary_path",
        );
        _adjust(
            root_dir,
            indexer_dir.as_ref(),
            &mut self.rich_indexer.store,
            "sqlite/sqlite.db",
        );
    }
}

fn _adjust(root_dir: &Path, indexer_dir: &Path, target: &mut PathBuf, sub: &str) {
    if target.to_str().is_none() || target.to_str() == Some("") {
        *target = indexer_dir.to_path_buf().join(sub);
    } else if target.is_relative() {
        *target = root_dir.to_path_buf().join(&target)
    }
}

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
}

impl From<&IndexerConfig> for IndexerSyncConfig {
    fn from(config: &IndexerConfig) -> IndexerSyncConfig {
        IndexerSyncConfig {
            secondary_path: config.secondary_path.clone(),
            poll_interval: config.poll_interval,
            index_tx_pool: config.index_tx_pool,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `sec-desk` report 638: an enabled indexer used to be unbounded unless the operator
    // explicitly set `request_limit`. The default must cap a request even when the whole
    // `[indexer_v2]` section is missing from `ckb.toml`.
    #[test]
    fn default_indexer_config_caps_requests() {
        let config = IndexerConfig::default();
        assert_eq!(config.request_limit, Some(DEFAULT_REQUEST_LIMIT));
        assert_eq!(config.timeout_limit, Some(DEFAULT_TIMEOUT_LIMIT));
    }

    // A section that exists but omits the two keys must get the same caps: serde's field
    // defaults are what actually apply when `[indexer_v2]` is present without them.
    #[test]
    fn indexer_section_without_limits_uses_defaults() {
        let config: IndexerConfig = toml::from_str(
            r#"
            index_tx_pool = false
            "#,
        )
        .expect("parse indexer config");

        assert_eq!(config.request_limit, Some(DEFAULT_REQUEST_LIMIT));
        assert_eq!(config.timeout_limit, Some(DEFAULT_TIMEOUT_LIMIT));
    }

    #[test]
    fn explicit_limits_are_honoured() {
        let config: IndexerConfig = toml::from_str(
            r#"
            request_limit = 5000
            timeout_limit = 30
            "#,
        )
        .expect("parse indexer config");

        assert_eq!(config.request_limit, Some(5000));
        assert_eq!(config.timeout_limit, Some(30));
    }
}
