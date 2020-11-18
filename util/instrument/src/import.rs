use ckb_chain::chain::ChainController;
use ckb_jsonrpc_types::BlockView as JsonBlock;
use ckb_types::core;
#[cfg(feature = "progress_bar")]
use indicatif::{ProgressBar, ProgressStyle};
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
}

impl Import {
    /// TODO(doc): @doitian
    pub fn new(chain: ChainController, source: PathBuf) -> Self {
        Import { chain, source }
    }

    /// TODO(doc): @doitian
    pub fn execute(self) -> Result<(), Box<dyn Error>> {
        self.read_from_json()
    }

    #[cfg(not(feature = "progress_bar"))]
    pub fn read_from_json(&self) -> Result<(), Box<dyn Error>> {
        let f = fs::File::open(&self.source)?;
        let reader = io::BufReader::new(f);

        for line in reader.lines() {
            let s = line?;
            let block: JsonBlock = serde_json::from_str(&s)?;
            let block: Arc<core::BlockView> = Arc::new(block.into());
            if !block.is_genesis() {
                self.chain
                    .process_block(block)
                    .expect("import occur malformation data");
            }
        }
        Ok(())
    }

    /// TODO(doc): @doitian
    #[cfg(feature = "progress_bar")]
    pub fn read_from_json(&self) -> Result<(), Box<dyn Error>> {
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
            let block: JsonBlock = serde_json::from_str(&s)?;
            let block: Arc<core::BlockView> = Arc::new(block.into());
            if !block.is_genesis() {
                self.chain
                    .process_block(block)
                    .expect("import occur malformation data");
            }
            progress_bar.inc(s.as_bytes().len() as u64);
        }
        progress_bar.finish_with_message("done!");
        Ok(())
    }
}
