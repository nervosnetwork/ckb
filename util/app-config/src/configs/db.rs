use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Database config options.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Database directory path.
    ///
    /// By default, it is a subdirectory inside the data directory.
    #[serde(default)]
    pub path: PathBuf,
    /// The capacity of RocksDB cache, which caches uncompressed data blocks, indexes and filters, default is 128MB
    #[serde(default)]
    pub cache_size: Option<usize>,
    /// Provide RocksDB options.
    ///
    /// More details can be found in [the official tuning guide](https://github.com/facebook/rocksdb/wiki/RocksDB-Tuning-Guide).
    #[serde(default)]
    pub options: HashMap<String, String>,
    /// Provide an options file to tune RocksDB for your workload and your system configuration.
    ///
    /// More details can be found in [the official tuning guide](https://github.com/facebook/rocksdb/wiki/RocksDB-Tuning-Guide).
    pub options_file: Option<PathBuf>,
}

impl Config {
    /// Canonicalizes paths in the config options.
    ///
    /// If `self.path` is not set, set it to `data_dir / name`.
    ///
    /// If `self.path` or `self.options_file` is relative, convert them to absolute path using
    /// `root_dir` as current working directory.
    pub fn adjust<P: AsRef<Path>>(&mut self, root_dir: &Path, data_dir: P, name: &str) {
        // If path is not set, use the default path
        if self.path.to_str().is_none() || self.path.to_str() == Some("") {
            self.path = data_dir.as_ref().to_path_buf().join(name);
        } else if self.path.is_relative() {
            // If the path is relative, set the base path to `ckb.toml`
            self.path = root_dir.to_path_buf().join(&self.path)
        }
        // If options file is a relative path, set the base path to `ckb.toml`
        if let Some(file) = self.options_file.iter_mut().next() {
            if file.is_relative() {
                let file_new = root_dir.to_path_buf().join(&file);
                *file = file_new;
            }
        }
    }
}
