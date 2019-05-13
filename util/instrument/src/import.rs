use crate::format::Format;
use ckb_chain::chain::ChainController;
use ckb_core::block::Block;
#[cfg(feature = "progress_bar")]
use indicatif::{ProgressBar, ProgressStyle};
use serde_json;
use std::error::Error;
use std::fs;
use std::io;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::Arc;

/// Export block date from file to database.
pub struct Import {
    /// source file contains block data
    source: PathBuf,
    chain: ChainController,
    /// source file format
    format: Format,
}

impl Import {
    pub fn new(chain: ChainController, format: Format, source: PathBuf) -> Self {
        Import {
            format,
            chain,
            source,
        }
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
            let block: Arc<Block> = Arc::new(serde_json::from_str(&s)?);
            if !block.is_genesis() {
                self.chain
                    .process_block(block, true)
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
            let block: Arc<Block> = Arc::new(serde_json::from_str(&s)?);
            if !block.is_genesis() {
                self.chain
                    .process_block(block, true)
                    .expect("import occur malformation data");
            }
            progress_bar.inc(s.as_bytes().len() as u64);
        }
        progress_bar.finish_with_message("done!");
        Ok(())
    }
}
