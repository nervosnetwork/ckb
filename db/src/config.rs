use serde_derive::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize)]
pub struct DBConfig {
    pub path: PathBuf,
    pub rocksdb: Option<HashMap<String, String>>,
}

impl Default for DBConfig {
    fn default() -> Self {
        DBConfig {
            path: Default::default(),
            rocksdb: None,
        }
    }
}
