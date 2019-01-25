use std::path::PathBuf;
use crate::kvdb::KeyValueDB;
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
}

impl RocksDBConfig {
    pub fn to_options(&self) -> Options {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        opts
    }
}
