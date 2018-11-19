use super::format::Format;
use ckb_chain::chain::ChainProvider;
use ckb_core::block::Block;
#[cfg(feature = "progress_bar")]
use indicatif::{ProgressBar, ProgressStyle};
use serde_json;
use std::error::Error;
use std::fs;
use std::io;
use std::io::BufRead;
use std::path::PathBuf;

/// Export block date from file to database.
pub struct Import<'a, P: 'a> {
    /// source file contains block data
    pub source: PathBuf,
    pub provider: &'a P,
    /// source file format
    pub format: Format,
}

impl<'a, P: ChainProvider> Import<'a, P> {
    pub fn new(provider: &'a P, format: Format, source: PathBuf) -> Self {
        Import {
            format,
            provider,
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
            let encoded: Block = serde_json::from_str(&s)?;
            let block: Block = encoded.into();
            if !block.is_genesis() {
                self.provider
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
            let block: Block = serde_json::from_str(&s)?;
            if !block.is_genesis() {
                self.provider
                    .process_block(&block)
                    .expect("import occur malformation data");
            }
            progress_bar.inc(s.as_bytes().len() as u64);
        }
        progress_bar.finish_with_message("done!");
        Ok(())
    }
}
