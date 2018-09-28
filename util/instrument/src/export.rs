use super::format::Format;
use super::iter::ChainIterator;
use ckb_chain::cachedb::CacheDB;
use ckb_chain::chain::{Chain, ChainBuilder};
use ckb_chain::store::ChainKVStore;
use ckb_chain_spec::{ChainSpec, SpecType};
use ckb_core::block::Block;
use ckb_db::diskdb::RocksDB;
use dir::Directories;
#[cfg(feature = "progress_bar")]
use indicatif::{ProgressBar, ProgressStyle};
use serde_json;
use std::error::Error;
use std::fs;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Export block from datbase to specify file.
pub struct Export {
    /// chain Specification
    pub spec: ChainSpec,

    /// node directories, provide ckb node directory structure
    pub dirs: Directories,

    /// export target path
    pub target: PathBuf,

    pub chain: Chain<ChainKVStore<CacheDB<RocksDB>>>,

    /// which format be used to export
    pub format: Format,
}

impl Export {
    pub fn new<P: AsRef<Path>>(
        base_path: P,
        format: Format,
        target: PathBuf,
        spec_type: &str,
    ) -> Result<Self, Box<Error>> {
        let dirs = Directories::new(base_path.as_ref());
        let db_path = dirs.join("db");

        let spec_type: SpecType = spec_type.parse()?;
        let spec = spec_type.load_spec()?;

        let builder = ChainBuilder::<ChainKVStore<CacheDB<RocksDB>>>::new_rocks(&db_path)
            .consensus(spec.to_consensus()?);
        let chain = builder.build().unwrap();

        Ok(Export {
            dirs,
            format,
            target,
            chain,
            spec,
        })
    }

    /// Returning ChainIterator dealing with blocks iterate.
    pub fn iter<'a>(&'a self) -> ChainIterator<'a, Chain<ChainKVStore<CacheDB<RocksDB>>>> {
        ChainIterator::new(&self.chain)
    }

    /// export file name
    fn file_name(&self) -> String {
        format!("{}.{}", self.spec.name, self.format)
    }

    pub fn execute(self) -> Result<(), Box<Error>> {
        fs::create_dir_all(&self.target)?;
        match self.format {
            Format::Json => self.write_to_json(),
            _ => Ok(()),
        }
    }

    #[cfg(not(feature = "progress_bar"))]
    pub fn write_to_json(self) -> Result<(), Box<Error>> {
        let f = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&self.target.join(self.file_name()))?;
        let mut writer = io::BufWriter::new(f);

        for block in self.iter() {
            let block: Block = block.into();
            let encoded = serde_json::to_vec(&block)?;
            writer.write_all(&encoded)?;
            writer.write_all(b"\n")?;
        }
        Ok(())
    }

    #[cfg(feature = "progress_bar")]
    pub fn write_to_json(self) -> Result<(), Box<Error>> {
        let f = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&self.target.join(self.file_name()))?;
        let mut writer = io::BufWriter::new(f);

        let blocks_iter = self.iter();
        let progress_bar = ProgressBar::new(blocks_iter.len());
        progress_bar.set_style(
            ProgressStyle::default_bar()
                .template("[{elapsed_precise}] {bar:50.cyan/blue} {pos:>6}/{len:6} {msg}")
                .progress_chars("##-"),
        );
        for block in blocks_iter {
            let block: Block = block.into();
            let encoded = serde_json::to_vec(&block)?;
            writer.write_all(&encoded)?;
            writer.write_all(b"\n")?;
            progress_bar.inc(1);
        }
        progress_bar.finish_with_message("done!");
        Ok(())
    }
}
