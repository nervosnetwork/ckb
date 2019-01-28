use serde_derive::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize)]
pub struct DBConfig {
    pub path: PathBuf,
    pub backend: String, // "memory" or "rocksdb"
    pub rocksdb: Option<HashMap<String, String>>,
}

impl Default for DBConfig {
    fn default() -> Self {
        DBConfig {
            path: Default::default(),
            backend: "rocksdb".to_owned(),
            rocksdb: None,
        }
    }
}
