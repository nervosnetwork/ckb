extern crate app_dirs;

use app_dirs::{get_app_root, AppDataType, AppInfo};
use std::path::{Path, PathBuf};
use std::{env, fs};

const APP_INFO: AppInfo = AppInfo {
    name: "nervos",
    author: "Nervos Dev",
};

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
        let result = self.base.join(path);
        fs::create_dir_all(result.clone()).expect("Unable to create dir");
        result
    }
}

/// Default data path
pub fn default_base_path() -> PathBuf {
    get_app_root(AppDataType::UserData, &APP_INFO).unwrap_or_else(|_| home().join(".nervos"))
}

/// Get home directory.
fn home() -> PathBuf {
    env::home_dir().expect("Failed to get home dir")
}

#[cfg(test)]
mod tests {
    use super::default_base_path;

    #[test]
    fn test_default_base_path() {
        println!("{:?}", default_base_path());
    }
}
