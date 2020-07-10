use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub path: PathBuf,
    pub options: Option<HashMap<String, String>>,
}

impl Config {
    pub fn update_path_if_not_set<P: AsRef<Path>>(&mut self, path: P) {
        if self.path.to_str().is_none() || self.path.to_str() == Some("") {
            self.path = path.as_ref().to_path_buf();
        }
    }
}
