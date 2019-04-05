use serde_derive::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Default)]
pub struct DBConfig {
    #[serde(default)]
    pub path: PathBuf,
    pub options: Option<HashMap<String, String>>,
}
