// Shields clippy errors in generated bundled.rs
#![allow(clippy::unreadable_literal)]

mod template;

pub use self::template::{
    TemplateContext, AVAILABLE_SPECS, DEFAULT_P2P_PORT, DEFAULT_RPC_PORT, DEFAULT_SPEC,
};
pub use std::io::{Error, Result};

use self::template::Template;
use std::borrow::Cow;
use std::fmt;
use std::fs;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

include!(concat!(env!("OUT_DIR"), "/bundled.rs"));

pub const CKB_CONFIG_FILE_NAME: &str = "ckb.toml";
pub const MINER_CONFIG_FILE_NAME: &str = "ckb-miner.toml";
pub const SPEC_DEV_FILE_NAME: &str = "specs/dev.toml";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Resource {
    Bundled(String),
    FileSystem(PathBuf),
}

impl fmt::Display for Resource {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Resource::Bundled(key) => write!(f, "Bundled({})", key),
            Resource::FileSystem(path) => write!(f, "FileSystem({})", path.display()),
        }
    }
}

impl Resource {
    pub fn is_bundled(&self) -> bool {
        match self {
            Resource::Bundled(_) => true,
            _ => false,
        }
    }

    /// Gets resource content
    pub fn get(&self) -> Result<Cow<'static, [u8]>> {
        match self {
            Resource::Bundled(key) => BUNDLED.get(key),
            Resource::FileSystem(path) => {
                let mut file = BufReader::new(fs::File::open(path)?);
                let mut data = Vec::new();
                file.read_to_end(&mut data)?;
                Ok(Cow::Owned(data))
            }
        }
    }

    /// Gets resource input stream
    pub fn read(&self) -> Result<Box<dyn Read>> {
        match self {
            Resource::Bundled(key) => BUNDLED.read(key),
            Resource::FileSystem(path) => Ok(Box::new(BufReader::new(fs::File::open(path)?))),
        }
    }
}

pub struct ResourceLocator {
    root_dir: PathBuf,
}

impl ResourceLocator {
    pub fn root_dir(&self) -> &Path {
        self.root_dir.as_path()
    }

    /// Creates a ResourceLocator using `path` as root directory.
    ///
    /// It returns error if the directory does not exists and current user has no permission to create it.
    pub fn with_root_dir(root_dir: PathBuf) -> Result<ResourceLocator> {
        fs::create_dir_all(&root_dir)?;

        Ok(ResourceLocator { root_dir })
    }

    pub fn current_dir() -> Result<ResourceLocator> {
        let root_dir = ::std::env::current_dir()?;

        Ok(ResourceLocator { root_dir })
    }

    pub fn ckb(&self) -> Resource {
        self.resolve(PathBuf::from(CKB_CONFIG_FILE_NAME)).unwrap()
    }

    pub fn miner(&self) -> Resource {
        self.resolve(PathBuf::from(MINER_CONFIG_FILE_NAME)).unwrap()
    }

    /// Resolves a resource using a path.
    ///
    /// The path may be absolute or relative. This function tries the file system first. If the file
    /// is absent in the file system and it is relative, the function will search in the bundled files.
    ///
    /// The relative path is relative to the resource root directory.
    ///
    /// All the bundled files are assumed in the resource root directory.
    ///
    /// It returns None when no resource with the path is found.
    pub fn resolve(&self, path: PathBuf) -> Option<Resource> {
        if path.is_absolute() {
            return file_system(path);
        }

        file_system(self.root_dir.join(&path)).or_else(|| bundled(path))
    }

    /// Resolves a resource using a path as the path is refered in the resource `relative_to`.
    ///
    /// This function is similar to [`ResourceLocator::resolve`]. The difference is how to resolve a relative path.
    ///
    /// [`ResourceLocator::resolve`]: struct.ResourceLocator.html#method.open
    ///
    /// The relative path is relative to the directory containing the resource `relative_to`.
    ///
    /// For security reason, when `relative_to` is `Resource::Bundled`, the return value is either
    /// `Some(Resource::Bundled)` or `None`. A bundled file is forbidden to reference a file in the
    /// file system.
    pub fn resolve_relative_to(&self, path: PathBuf, relative_to: &Resource) -> Option<Resource> {
        match relative_to {
            Resource::Bundled(key) => {
                // Bundled file can only refer to bundled files.
                let relative_start_dir = parent_dir(PathBuf::from(key)).join(&path);
                bundled(relative_start_dir)
            }
            Resource::FileSystem(relative_to_path) => {
                if path.is_absolute() {
                    return file_system(path);
                }

                let start_dir = parent_dir(relative_to_path.clone());
                file_system(start_dir.join(&path)).or_else(|| {
                    start_dir
                        .strip_prefix(&self.root_dir)
                        .ok()
                        .and_then(|relative_start_dir| bundled(relative_start_dir.join(path)))
                })
            }
        }
    }

    pub fn exported(&self) -> bool {
        BUNDLED
            .file_names()
            .any(|name| self.root_dir.join(name).exists())
    }

    pub fn export_ckb<'a>(&self, context: &TemplateContext<'a>) -> Result<()> {
        self.export(CKB_CONFIG_FILE_NAME, context)
    }

    pub fn export_miner<'a>(&self, context: &TemplateContext<'a>) -> Result<()> {
        self.export(MINER_CONFIG_FILE_NAME, context)
    }

    pub fn export<'a>(&self, name: &str, context: &TemplateContext<'a>) -> Result<()> {
        let target = self.root_dir.join(name);
        let resource = Resource::Bundled(name.to_string());
        let template = Template::new(from_utf8(resource.get()?)?);
        let mut out = NamedTempFile::new_in(&self.root_dir)?;
        if name.contains('/') {
            if let Some(dir) = target.parent() {
                fs::create_dir_all(dir)?;
            }
        }
        template.write_to(&mut out, context)?;
        out.persist(target)?;
        Ok(())
    }
}

#[cfg(windows)]
fn path_as_key(path: &PathBuf) -> Cow<str> {
    Cow::Owned(path.to_string_lossy().replace("\\", "/"))
}

#[cfg(not(windows))]
fn path_as_key(path: &PathBuf) -> Cow<str> {
    path.to_string_lossy()
}

fn file_system(path: PathBuf) -> Option<Resource> {
    if path.exists() {
        Some(Resource::FileSystem(path))
    } else {
        None
    }
}

pub fn bundled(path: PathBuf) -> Option<Resource> {
    let key = path_as_key(&path);
    if BUNDLED.is_available(&key) {
        Some(Resource::Bundled(key.into_owned()))
    } else {
        None
    }
}

fn parent_dir(mut path: PathBuf) -> PathBuf {
    path.pop();
    path
}

fn from_utf8(data: Cow<[u8]>) -> Result<String> {
    String::from_utf8(data.to_vec()).map_err(|err| Error::new(io::ErrorKind::Other, err))
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
    fn test_resource_locator_resolve() {
        let dir = mkdir();
        let spec_dev_path = touch(dir.path().join("specs/dev.toml"));

        let locator = ResourceLocator::with_root_dir(dir.path().to_path_buf())
            .expect("resource root dir exists");

        assert_eq!(
            locator.resolve("ckb.toml".into()),
            Some(Resource::Bundled("ckb.toml".into()))
        );

        assert_eq!(
            locator.resolve("specs/testnet.toml".into()),
            Some(Resource::Bundled("specs/testnet.toml".into()))
        );
        assert_eq!(
            locator.resolve("specs/dev.toml".into()),
            Some(Resource::FileSystem(spec_dev_path.clone()))
        );

        assert_eq!(locator.resolve(dir.path().join("ckb.toml")), None);
        assert_eq!(locator.resolve("x.toml".into()), None);
    }

    #[test]
    fn test_resource_locator_resolve_relative_to() {
        let dir = mkdir();
        let spec_dev_path = touch(dir.path().join("specs/dev.toml"));

        let locator = ResourceLocator::with_root_dir(dir.path().to_path_buf())
            .expect("resource root dir exists");

        // Relative to Bundled("ckb.toml")
        {
            let ckb = Resource::Bundled("ckb.toml".into());

            assert_eq!(
                locator.resolve_relative_to("specs/dev.toml".into(), &ckb),
                Some(Resource::Bundled("specs/dev.toml".into()))
            );
            assert_eq!(
                locator.resolve_relative_to("specs/testnet.toml".into(), &ckb),
                Some(Resource::Bundled("specs/testnet.toml".into()))
            );
            assert_eq!(locator.resolve_relative_to("x".into(), &ckb), None);
            assert_eq!(
                locator.resolve_relative_to("cells/always_success".into(), &ckb),
                None,
            );
            assert_eq!(
                locator.resolve_relative_to(spec_dev_path.clone(), &ckb),
                None,
            );
        }

        // Relative to Bundled("specs/dev.toml")
        {
            let ckb = Resource::Bundled("specs/dev.toml".into());

            assert_eq!(
                locator.resolve_relative_to("cells/secp256k1_blake160_sighash_all".into(), &ckb),
                Some(Resource::Bundled(
                    "specs/cells/secp256k1_blake160_sighash_all".into()
                ))
            );
            assert_eq!(locator.resolve_relative_to("x".into(), &ckb), None);
            assert_eq!(
                locator.resolve_relative_to("cells/always_success".into(), &ckb),
                None,
            );
        }

        // Relative to FileSystem("specs/dev.toml")
        {
            let spec_dev = Resource::FileSystem(spec_dev_path.clone());

            assert_eq!(
                locator
                    .resolve_relative_to("cells/secp256k1_blake160_sighash_all".into(), &spec_dev),
                Some(Resource::Bundled(
                    "specs/cells/secp256k1_blake160_sighash_all".into()
                ))
            );
            assert_eq!(locator.resolve_relative_to("x".into(), &spec_dev), None);

            assert_eq!(
                locator.resolve_relative_to("cells/always_success".into(), &spec_dev),
                None,
            );
            assert_eq!(
                locator.resolve_relative_to(dir.path().join("ckb.toml"), &spec_dev),
                None,
            );
        }
    }
}
