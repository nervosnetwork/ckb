use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexerConfig {
    pub store: PathBuf,
    pub secondary_path: PathBuf,
    /// The poll interval by secs
    pub poll_interval: u64,
    /// Whether to index the pending txs in the ckb tx-pool
    pub index_tx_pool: bool,
}

impl Default for IndexerConfig {
    fn default() -> Self {
        IndexerConfig {
            poll_interval: 2,
            index_tx_pool: false,
            store: PathBuf::new(),
            secondary_path: PathBuf::new(),
        }
    }
}

impl IndexerConfig {
    /// Canonicalizes paths in the config options.
    ///
    /// If `self.store` is not set, set it to `data_dir / indexer / store`.
    ///
    /// If `self.secondary_path` is not set, set it to `data_dir / indexer / secondary_path`.
    ///.
    /// If `self.path` is relative, convert them to absolute path using
    /// `root_dir` as current working directory.
    pub fn adjust<P: AsRef<Path>>(&mut self, root_dir: &Path, indexer_dir: P) {
        _adjust(root_dir, indexer_dir.as_ref(), &mut self.store, "store");
        _adjust(
            root_dir,
            indexer_dir.as_ref(),
            &mut self.secondary_path,
            "secondary_path",
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
