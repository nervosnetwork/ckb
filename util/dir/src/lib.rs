use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Clone)]
pub struct Directories {
    pub base: PathBuf,
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
