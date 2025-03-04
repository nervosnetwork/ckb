//! Bundles resources in the ckb binary.
//!
//! This crate bundles the files ckb.toml, ckb-miner.toml, default.db-options, and all files in the
//! directory `specs` in the binary.
//!
//! The bundled files can be read via `Resource::Bundled`, for example:
//!
//! ```
//! // Read bundled ckb.toml
//! use ckb_resource::{Resource, CKB_CONFIG_FILE_NAME};
//!
//! let ckb_toml_bytes = Resource::bundled(CKB_CONFIG_FILE_NAME.to_string()).get().unwrap();
//! println!("ckb.toml\n{}", String::from_utf8(ckb_toml_bytes.to_vec()).unwrap());
//! ```
//!
//! These bundled files can be customized for different chains using spec branches.
//! See [Template](struct.Template.html).

mod template;

#[cfg(test)]
mod tests;

pub use self::template::Template;
pub use self::template::{
    AVAILABLE_SPECS, DEFAULT_P2P_PORT, DEFAULT_RPC_PORT, DEFAULT_SPEC, TemplateContext,
};
pub use std::io::{Error, Result};

use ckb_types::H256;
use includedir::Files;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fmt;
use std::fs;
use std::io::{self, BufReader, Cursor, Read};
use std::path::{Path, PathBuf};

use ckb_system_scripts::BUNDLED_CELL;

mod bundled {
    #![allow(missing_docs, clippy::unreadable_literal)]
    include!(concat!(env!("OUT_DIR"), "/bundled.rs"));
}
/// Bundled resources in ckb binary.
pub use bundled::BUNDLED;

include!(concat!(env!("OUT_DIR"), "/code_hashes.rs"));

/// CKB config file name.
pub const CKB_CONFIG_FILE_NAME: &str = "ckb.toml";
/// CKB miner config file name.
pub const MINER_CONFIG_FILE_NAME: &str = "ckb-miner.toml";
/// The relative spec file path for the dev chain.
pub const SPEC_DEV_FILE_NAME: &str = "specs/dev.toml";
/// The file name of the generated RocksDB options file.
pub const DB_OPTIONS_FILE_NAME: &str = "default.db-options";

/// Represents a resource, which is either bundled in the CKB binary or resident in the local file
/// system.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Resource {
    /// A resource that bundled in the CKB binary.
    Bundled {
        /// The identifier of the bundled resource.
        bundled: String,
    },
    /// A resource that resides in the local file system.
    FileSystem {
        /// The file path to the resource.
        file: PathBuf,
    },
    /// A resource that init by user custom
    Raw {
        /// raw data
        raw: String,
    },
}

impl fmt::Display for Resource {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Resource::Bundled { bundled } => write!(f, "Bundled({bundled})"),
            Resource::FileSystem { file } => write!(f, "FileSystem({})", file.display()),
            Resource::Raw { raw } => write!(f, "Raw({})", raw),
        }
    }
}

impl Resource {
    /// Creates a reference to the bundled resource.
    pub fn bundled(bundled: String) -> Resource {
        Resource::Bundled { bundled }
    }

    /// Creates a reference to the resource resident in the file system.
    pub fn file_system(file: PathBuf) -> Resource {
        Resource::FileSystem { file }
    }

    /// Creates a reference to the resource resident in the memory.
    pub fn raw(raw: String) -> Resource {
        Resource::Raw { raw }
    }

    /// Creates the CKB config file resource from the file system.
    ///
    /// It searches the file name `CKB_CONFIG_FILE_NAME` in the directory `root_dir`.
    pub fn ckb_config<P: AsRef<Path>>(root_dir: P) -> Resource {
        Resource::file_system(root_dir.as_ref().join(CKB_CONFIG_FILE_NAME))
    }

    /// Creates the CKB miner config file resource from the file system.
    ///
    /// It searches the file name `MINER_CONFIG_FILE_NAME` in the directory `root_dir`.
    pub fn miner_config<P: AsRef<Path>>(root_dir: P) -> Resource {
        Resource::file_system(root_dir.as_ref().join(MINER_CONFIG_FILE_NAME))
    }

    /// Creates the RocksDB options file resource from the file system.
    ///
    /// It searches the file name `DB_OPTIONS_FILE_NAME` in the directory `root_dir`.
    pub fn db_options<P: AsRef<Path>>(root_dir: P) -> Resource {
        Resource::file_system(root_dir.as_ref().join(DB_OPTIONS_FILE_NAME))
    }

    /// Creates the bundled CKB config file resource.
    pub fn bundled_ckb_config() -> Resource {
        Resource::bundled(CKB_CONFIG_FILE_NAME.to_string())
    }

    /// Creates the bundled CKB miner config file resource.
    pub fn bundled_miner_config() -> Resource {
        Resource::bundled(MINER_CONFIG_FILE_NAME.to_string())
    }

    /// Creates the bundled RocksDB options file resource.
    pub fn bundled_db_options() -> Resource {
        Resource::bundled(DB_OPTIONS_FILE_NAME.to_string())
    }

    /// Checks whether any of the bundled resource has been exported in the specified directory.
    ///
    /// This can be used to avoid overwriting to export all the bundled resources to the specified
    /// directory.
    pub fn exported_in<P: AsRef<Path>>(root_dir: P) -> bool {
        BUNDLED
            .file_names()
            .chain(BUNDLED_CELL.file_names())
            .any(|name| join_bundled_key(root_dir.as_ref().to_path_buf(), name).exists())
    }

    /// Returns `true` if this is a bundled resource.
    pub fn is_bundled(&self) -> bool {
        matches!(self, Resource::Bundled { .. })
    }

    /// Returns `true` if the resource exists.
    ///
    /// The bundled resource exists only when the identifier is included in the bundle.
    ///
    /// The file system resource exists only when the file exists.
    pub fn exists(&self) -> bool {
        match self {
            Resource::Bundled { bundled } => {
                SourceFiles::new(&BUNDLED_CELL, &BUNDLED).is_available(bundled)
            }
            Resource::FileSystem { file } => file.exists(),
            Resource::Raw { .. } => true,
        }
    }

    /// The parent directory of the resource.
    ///
    /// It always returns `None` on bundled resource.
    pub fn parent(&self) -> Option<&Path> {
        match self {
            Resource::FileSystem { file } => file.parent(),
            _ => None,
        }
    }

    /// Modifies the file system resource to ensure the path is absolute.
    ///
    /// If the path is relative, expand the path relative to the directory `base`.
    pub fn absolutize<P: AsRef<Path>>(&mut self, base: P) {
        if let Resource::FileSystem { file: path } = self {
            if path.is_relative() {
                *path = base.as_ref().join(&path)
            }
        }
    }

    /// Gets resource content.
    pub fn get(&self) -> Result<Cow<'static, [u8]>> {
        match self {
            Resource::Bundled { bundled } => SourceFiles::new(&BUNDLED_CELL, &BUNDLED).get(bundled),
            Resource::FileSystem { file } => Ok(Cow::Owned(fs::read(file)?)),
            Resource::Raw { raw } => Ok(Cow::Owned(raw.to_owned().into_bytes())),
        }
    }

    /// Gets resource content via an input stream.
    pub fn read(&self) -> Result<Box<dyn Read>> {
        match self {
            Resource::Bundled { bundled } => {
                SourceFiles::new(&BUNDLED_CELL, &BUNDLED).read(bundled)
            }
            Resource::FileSystem { file } => Ok(Box::new(BufReader::new(fs::File::open(file)?))),
            Resource::Raw { raw } => Ok(Box::new(Cursor::new(raw.to_owned().into_bytes()))),
        }
    }

    /// Exports a bundled resource.
    ///
    /// This function returns `Ok` immediately when invoked on a file system resource.
    ///
    /// The file is exported to the path by combining `root_dir` and the resource identifier.
    ///
    /// These bundled files can be customized for different chains using spec branches.
    /// See [Template](struct.Template.html).
    pub fn export<P: AsRef<Path>>(&self, context: &TemplateContext<'_>, root_dir: P) -> Result<()> {
        let key = match self {
            Resource::Bundled { bundled } => bundled,
            _ => return Ok(()),
        };
        let target = join_bundled_key(root_dir.as_ref().to_path_buf(), key);
        let template = Template::new(from_utf8(self.get()?)?);
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut f = fs::File::create(&target)?;
        template.render_to(&mut f, context)?;
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
