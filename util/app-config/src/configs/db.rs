use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub path: PathBuf,
    #[serde(default)]
    pub options: Option<HashMap<String, String>>,
}
