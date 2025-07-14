use ckb_chain::ChainController;
use ckb_jsonrpc_types::BlockView as JsonBlock;
use ckb_types::core;
use ckb_verification_traits::Switch;
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
    switch: Switch,
}

impl Import {
    /// Creates a new import job.
    pub fn new(chain: ChainController, source: PathBuf, switch: Switch) -> Self {
        Import {
            chain,
            source,
            switch,
        }
    }

    /// Executes the import job.
    pub fn execute(self) -> Result<(), Box<dyn Error>> {
        self.read_from_json()
    }

    /// Imports the chain from the JSON file.
    #[cfg(feature = "progress_bar")]
    pub fn read_from_json(&self) -> Result<(), Box<dyn Error>> {
        let f = fs::File::open(&self.source)?;
        let reader = io::BufReader::new(f);

        #[cfg(feature = "progress_bar")]
        let progress_bar = ProgressBar::new(fs::metadata(&self.source)?.len());
        #[cfg(feature = "progress_bar")]
        progress_bar.set_style(
            ProgressStyle::default_bar()
                .template("[{elapsed_precise}] {bar:50.cyan/blue} {bytes:>6}/{total_bytes:6} {msg}")
                .expect("Failed to set progress bar template")
                .progress_chars("##-"),
        );
        for line in reader.lines() {
            let s = line?;
            let block: JsonBlock = serde_json::from_str(&s)?;
            let block: Arc<core::BlockView> = Arc::new(block.into());
            if !block.is_genesis() {
                self.chain
                    .blocking_process_block_with_switch(block, self.switch)
                    .expect("import occur malformation data");
            }

            #[cfg(feature = "progress_bar")]
            progress_bar.inc(s.len() as u64);
        }
        #[cfg(feature = "progress_bar")]
        progress_bar.finish_with_message("done!");
        Ok(())
    }
}
