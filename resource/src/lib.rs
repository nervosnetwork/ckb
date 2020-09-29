// Shields clippy errors in generated bundled.rs
#![allow(clippy::unreadable_literal)]

mod template;

pub use self::template::{
    TemplateContext, AVAILABLE_SPECS, DEFAULT_P2P_PORT, DEFAULT_RPC_PORT, DEFAULT_SPEC,
};
pub use std::io::{Error, Result};

use self::template::Template;
use ckb_types::H256;
use includedir::Files;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fmt;
use std::fs;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

use ckb_system_scripts::BUNDLED_CELL;

include!(concat!(env!("OUT_DIR"), "/bundled.rs"));
include!(concat!(env!("OUT_DIR"), "/code_hashes.rs"));

pub const CKB_CONFIG_FILE_NAME: &str = "ckb.toml";
pub const MINER_CONFIG_FILE_NAME: &str = "ckb-miner.toml";
pub const SPEC_DEV_FILE_NAME: &str = "specs/dev.toml";
pub const DB_OPTIONS_FILE_NAME: &str = "default.db-options";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Resource {
    Bundled { bundled: String },
    FileSystem { file: PathBuf },
}

impl fmt::Display for Resource {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Resource::Bundled { bundled } => write!(f, "Bundled({})", bundled),
            Resource::FileSystem { file } => write!(f, "FileSystem({})", file.display()),
        }
    }
}

impl Resource {
    pub fn bundled(bundled: String) -> Resource {
        Resource::Bundled { bundled }
    }

    pub fn file_system(file: PathBuf) -> Resource {
        Resource::FileSystem { file }
    }

    pub fn ckb_config<P: AsRef<Path>>(root_dir: P) -> Resource {
        Resource::file_system(root_dir.as_ref().join(CKB_CONFIG_FILE_NAME))
    }

    pub fn miner_config<P: AsRef<Path>>(root_dir: P) -> Resource {
        Resource::file_system(root_dir.as_ref().join(MINER_CONFIG_FILE_NAME))
    }

    pub fn db_options<P: AsRef<Path>>(root_dir: P) -> Resource {
        Resource::file_system(root_dir.as_ref().join(DB_OPTIONS_FILE_NAME))
    }

    pub fn bundled_ckb_config() -> Resource {
        Resource::bundled(CKB_CONFIG_FILE_NAME.to_string())
    }

    pub fn bundled_miner_config() -> Resource {
        Resource::bundled(MINER_CONFIG_FILE_NAME.to_string())
    }

    pub fn bundled_db_options() -> Resource {
        Resource::bundled(DB_OPTIONS_FILE_NAME.to_string())
    }

    pub fn exported_in<P: AsRef<Path>>(root_dir: P) -> bool {
        BUNDLED
            .file_names()
            .chain(BUNDLED_CELL.file_names())
            .any(|name| join_bundled_key(root_dir.as_ref().to_path_buf(), name).exists())
    }

    pub fn is_bundled(&self) -> bool {
        match self {
            Resource::Bundled { .. } => true,
            _ => false,
        }
    }

    pub fn exists(&self) -> bool {
        match self {
            Resource::Bundled { bundled } => {
                SourceFiles::new(&BUNDLED_CELL, &BUNDLED).is_available(bundled)
            }
            Resource::FileSystem { file } => file.exists(),
        }
    }

    pub fn parent(&self) -> Option<&Path> {
        match self {
            Resource::FileSystem { file } => file.parent(),
            _ => None,
        }
    }

    pub fn absolutize<P: AsRef<Path>>(&mut self, base: P) {
        if let Resource::FileSystem { file: ref mut path } = self {
            if path.is_relative() {
                *path = base.as_ref().join(&path)
            }
        }
    }

    /// Gets resource content
    pub fn get(&self) -> Result<Cow<'static, [u8]>> {
        match self {
            Resource::Bundled { bundled } => SourceFiles::new(&BUNDLED_CELL, &BUNDLED).get(bundled),
            Resource::FileSystem { file } => Ok(Cow::Owned(fs::read(file)?)),
        }
    }

    /// Gets resource input stream
    pub fn read(&self) -> Result<Box<dyn Read>> {
        match self {
            Resource::Bundled { bundled } => {
                SourceFiles::new(&BUNDLED_CELL, &BUNDLED).read(bundled)
            }
            Resource::FileSystem { file } => Ok(Box::new(BufReader::new(fs::File::open(file)?))),
        }
    }

    pub fn export<'a, P: AsRef<Path>>(
        &self,
        context: &TemplateContext<'a>,
        root_dir: P,
    ) -> Result<()> {
        let key = match self {
            Resource::Bundled { bundled } => bundled,
            _ => return Ok(()),
        };
        let target = join_bundled_key(root_dir.as_ref().to_path_buf(), key);
        let template = Template::new(from_utf8(self.get()?)?);
        let mut out = NamedTempFile::new_in(root_dir.as_ref())?;
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir)?;
        }
        template.write_to(&mut out, context)?;
        out.persist(target)?;
        Ok(())
    }
}

struct SourceFiles<'a> {
    system_cells: &'a Files,
    config: &'a Files,
}

impl<'a> SourceFiles<'a> {
    fn new(system_cells: &'a Files, config: &'a Files) -> Self {
        SourceFiles {
            system_cells,
            config,
        }
    }

    fn get(&self, path: &str) -> Result<Cow<'static, [u8]>> {
        self.config
            .get(path)
            .or_else(|_| self.system_cells.get(path))
    }

    fn read(&self, path: &str) -> Result<Box<dyn Read>> {
        self.config
            .read(path)
            .or_else(|_| self.system_cells.read(path))
    }

    fn is_available(&self, path: &str) -> bool {
        self.config.is_available(path) || self.system_cells.is_available(path)
    }
}

fn from_utf8(data: Cow<[u8]>) -> Result<String> {
    String::from_utf8(data.to_vec()).map_err(|err| Error::new(io::ErrorKind::Other, err))
}

fn join_bundled_key(mut root_dir: PathBuf, key: &str) -> PathBuf {
    key.split('/')
        .for_each(|component| root_dir.push(component));
    root_dir
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn mkdir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("ckb_resource_test")
            .tempdir()
            .unwrap()
    }

    fn touch<P: AsRef<Path>>(path: P) -> PathBuf {
        fs::create_dir_all(path.as_ref().parent().unwrap()).expect("create dir in test");
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("touch file in test");

        path.as_ref().to_path_buf()
    }

    #[test]
    fn test_exported_in() {
        let root_dir = mkdir();
        assert!(!Resource::exported_in(root_dir.path()));
        touch(root_dir.path().join(CKB_CONFIG_FILE_NAME));
        assert!(Resource::exported_in(root_dir.path()));
    }

    #[test]
    fn test_export() {
        let root_dir = mkdir();
        let context = TemplateContext::new(
            "dev",
            vec![
                ("rpc_port", "7000"),
                ("p2p_port", "8000"),
                ("log_to_file", "true"),
                ("log_to_stdout", "true"),
                ("block_assembler", ""),
                ("spec_source", "bundled"),
            ],
        );
        Resource::bundled_ckb_config()
            .export(&context, root_dir.path())
            .expect("export ckb.toml");
        assert!(Resource::exported_in(root_dir.path()));
    }
}
