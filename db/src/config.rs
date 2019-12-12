use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DBConfig {
    #[serde(default)]
    pub path: PathBuf,
    pub options: Option<HashMap<String, String>>,
}
