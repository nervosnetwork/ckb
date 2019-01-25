use std::path::PathBuf;
use serde_derive::Deserialize;
use rocksdb::Options;

#[derive(Clone, Debug, Deserialize)]
pub struct DBConfig {
    pub backend: String, // "memory" or "rocksdb"
    pub rocksdb: Option<RocksDBConfig>,
}

/// RocksDB specific db configurations.
///
/// https://docs.rs/rocksdb/0.6.0/rocksdb/struct.Options.html
#[derive(Clone, Debug, Deserialize)]
pub struct RocksDBConfig {
    pub path: PathBuf,
    pub create_if_missing: Option<bool>,
    pub create_missing_column_families: Option<bool>,
}

impl Default for RocksDBConfig {
    fn default() -> Self {
        RocksDBConfig {
            path: Default::default(),
            create_if_missing: None,
            create_missing_column_families: None,
        }
    }
}

impl RocksDBConfig {
    pub fn to_db_options(&self) -> Options {
        let mut opts = Options::default();

        opts.create_if_missing(
            self.create_if_missing.unwrap_or(true));
        opts.create_missing_column_families(
            self.create_missing_column_families.unwrap_or(true));

        opts
    }
}
