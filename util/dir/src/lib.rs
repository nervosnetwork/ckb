extern crate dirs;

use std::fs;
use std::path::{Path, PathBuf};

const APP_NANE: &str = "ckb";
const APP_AUTHOR: &str = "NervosDev";

#[derive(Debug, PartialEq, Clone)]
pub struct Directories {
    pub base: PathBuf,
}

impl Default for Directories {
    fn default() -> Self {
        let base = default_base_path();
        Directories::new(&base)
    }
}

impl Directories {
    pub fn new<P: AsRef<Path>>(base: P) -> Self {
        let base = base.as_ref().to_path_buf();
        Directories { base }
    }

    pub fn join<P: AsRef<Path>>(&self, path: P) -> PathBuf {
        let result = self.base.join(path.as_ref());
        fs::create_dir_all(&result).expect("Unable to create dir");
        result
    }
}

/// Default data path
pub fn default_base_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(home_dir)
        .join(APP_AUTHOR)
        .join(APP_NANE)
}

/// Get home directory.
fn home_dir() -> PathBuf {
    dirs::home_dir().expect("Failed to get home_dir")
}

#[cfg(test)]
mod tests {
    use super::default_base_path;

    #[test]
    fn test_default_base_path() {
        println!("{:?}", default_base_path());
    }
}
