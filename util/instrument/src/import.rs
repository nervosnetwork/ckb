use super::format::Format;
use ckb_chain::cachedb::CacheDB;
use ckb_chain::chain::ChainProvider;
use ckb_chain::chain::{Chain, ChainBuilder};
use ckb_chain::store::ChainKVStore;
use ckb_chain_spec::{ChainSpec, SpecType};
use ckb_core::block::{Block, IndexedBlock};
use ckb_db::diskdb::RocksDB;
use dir::Directories;
#[cfg(feature = "progress_bar")]
use indicatif::{ProgressBar, ProgressStyle};
use serde_json;
use std::error::Error;
use std::fs;
use std::io;
use std::io::BufRead;
use std::path::{Path, PathBuf};

/// Export block date from file to database.
pub struct Import {
    /// chain Specification
    pub spec: ChainSpec,

    /// node directories, provide ckb node directory structure
    pub dirs: Directories,

    /// source file contains block data
    pub source: PathBuf,
    pub chain: Chain<ChainKVStore<CacheDB<RocksDB>>>,

    /// source file format
    pub format: Format,
}

impl Import {
    pub fn new<P: AsRef<Path>>(
        base_path: P,
        format: Format,
        source: PathBuf,
        spec_type: &str,
    ) -> Result<Self, Box<Error>> {
        let dirs = Directories::new(base_path.as_ref());
        let db_path = dirs.join("db");

        let spec_type: SpecType = spec_type.parse()?;
        let spec = spec_type.load_spec()?;

        let builder = ChainBuilder::<ChainKVStore<CacheDB<RocksDB>>>::new_rocks(&db_path)
            .consensus(spec.to_consensus());
        let chain = builder.build().unwrap();

        Ok(Import {
            dirs,
            format,
            source,
            chain,
            spec,
        })
    }

    pub fn execute(self) -> Result<(), Box<Error>> {
        match self.format {
            Format::Json => self.read_from_json(),
            _ => Ok(()),
        }
    }

    #[cfg(not(feature = "progress_bar"))]
    pub fn read_from_json(&self) -> Result<(), Box<Error>> {
        let f = fs::File::open(&self.source)?;
        let reader = io::BufReader::new(f);

        for line in reader.lines() {
            let s = line?;
            let encoded: Block = serde_json::from_str(&s)?;
            let block: IndexedBlock = encoded.into();
            if !block.is_genesis() {
                self.chain
                    .process_block(&block)
                    .expect("import occur malformation data");
            }
        }
        Ok(())
    }

    #[cfg(feature = "progress_bar")]
    pub fn read_from_json(&self) -> Result<(), Box<Error>> {
        let metadata = fs::metadata(&self.source)?;
        let f = fs::File::open(&self.source)?;
        let reader = io::BufReader::new(f);
        let progress_bar = ProgressBar::new(metadata.len() as u64);
        progress_bar.set_style(
            ProgressStyle::default_bar()
                .template("[{elapsed_precise}] {bar:50.cyan/blue} {bytes:>6}/{total_bytes:6} {msg}")
                .progress_chars("##-"),
        );
        for line in reader.lines() {
            let s = line?;
            let encoded: Block = serde_json::from_str(&s)?;
            let block: IndexedBlock = encoded.into();
            if !block.is_genesis() {
                self.chain
                    .process_block(&block)
                    .expect("import occur malformation data");
            }
            progress_bar.inc(s.as_bytes().len() as u64);
        }
        progress_bar.finish_with_message("done!");
        Ok(())
    }
}
