use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// TODO(doc): @doitian
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    /// TODO(doc): @doitian
    #[serde(default)]
    pub path: PathBuf,
    /// TODO(doc): @doitian
    #[serde(default)]
    pub options: HashMap<String, String>,
    /// TODO(doc): @doitian
    pub options_file: Option<PathBuf>,
}

impl Config {
    /// TODO(doc): @doitian
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
