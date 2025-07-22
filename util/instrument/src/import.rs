use ckb_chain::ChainController;
use ckb_jsonrpc_types::BlockView as JsonBlock;
use ckb_shared::Shared;
use ckb_store::ChainStore;
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
    shared: Shared,
    chain: ChainController,
    switch: Switch,
}

impl Import {
    /// Creates a new import job.
    pub fn new(chain: ChainController, shared: Shared, source: PathBuf, switch: Switch) -> Self {
        Import {
            chain,
            shared,
            source,
            switch,
        }
    }

    /// Executes the import job.
    pub fn execute(self) -> Result<(), Box<dyn Error>> {
        {
            let snapshot = self.shared.snapshot();
            let tip = snapshot.tip_header();
            println!(
                "Before import, current tip is {}-{}",
                tip.number(),
                tip.hash()
            );
        }

        self.read_from_json()?;

        {
            let snapshot = self.shared.snapshot();
            let tip = snapshot.tip_header();
            println!(
                "After import, Current tip is {}-{}",
                tip.number(),
                tip.hash()
            );
        }
        Ok(())
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
        let mut first_block_checked = false;
        for line in reader.lines() {
            let s = line?;
            let block: JsonBlock = serde_json::from_str(&s)?;
            let block: Arc<core::BlockView> = Arc::new(block.into());

            if !block.is_genesis() {
                if !first_block_checked {
                    // check if first block's parent in db
                    let parent_hash = block.parent_hash();
                    if self.shared.snapshot().get_block(&parent_hash).is_none() {
                        let tip = self
                            .shared
                            .snapshot()
                            .get_tip_header()
                            .expect("must get tip header");

                        return Err(Box::new(io::Error::other(format!(
                            "In {}, the first block is {}-{}, and its parent (hash: {}) was not found in the database. The current tip is {}-{}.",
                            self.source.display(),
                            block.number(),
                            block.hash(),
                            parent_hash,
                            tip.number(),
                            tip.hash(),
                        ))));
                    }
                    first_block_checked = true;
                }

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
