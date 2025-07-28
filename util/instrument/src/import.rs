use ckb_chain::ChainController;
use ckb_jsonrpc_types::BlockView as JsonBlock;
use ckb_shared::Shared;
use ckb_store::ChainStore;
use ckb_types::core;
use ckb_verification_traits::Switch;
#[cfg(feature = "progress_bar")]
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
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
        use ckb_chain::VerifyResult;
        use ckb_types::core::BlockView;

        while self.chain.is_verifying_unverified_blocks_on_startup() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let f = fs::File::open(&self.source)?;
        let reader = io::BufReader::new(f);
        let mut lines = reader.lines().peekable();
        let first_block = if let Some(Ok(first_line)) = lines.peek() {
            let first_block: JsonBlock =
                serde_json::from_str(first_line).expect("parse first block from json");

            let first_block: core::BlockView = first_block.into();
            Ok(first_block)
        } else {
            Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidData,
                "The source file is empty or malformed.",
            )))
        }?;

        if !first_block.is_genesis() {
            let first_block_parent = first_block.parent_hash();
            if self
                .shared
                .snapshot()
                .get_block(&first_block_parent)
                .is_none()
            {
                let tip = self
                    .shared
                    .snapshot()
                    .get_tip_header()
                    .expect("must get tip header");

                return Err(Box::new(io::Error::other(format!(
                    "In {}, the first block is {}-{}, and its parent (hash: {}) was not found in the database. The current tip is {}-{}.",
                    self.source.display(),
                    first_block.number(),
                    first_block.hash(),
                    first_block_parent,
                    tip.number(),
                    tip.hash(),
                ))));
            }
        }

        #[cfg(feature = "progress_bar")]
        let progress_bar = ProgressBar::new(fs::metadata(&self.source)?.len());
        #[cfg(feature = "progress_bar")]
        progress_bar.set_style(
            ProgressStyle::default_bar()
                .template("[{elapsed_precise}] {bar:50.cyan/blue} {bytes:>6}/{total_bytes:6} {msg}")
                .expect("Failed to set progress bar template")
                .progress_chars("##-"),
        );

        let mut largest_block_number = 0;
        const BLOCKS_COUNT_PER_CHUNK: usize = 1024 * 6;
        let (blocks_tx, blocks_rx) = ckb_channel::bounded::<Arc<BlockView>>(BLOCKS_COUNT_PER_CHUNK);
        std::thread::spawn({
            #[cfg(feature = "progress_bar")]
            let progress_bar = progress_bar.clone();
            move || {
                let pool = rayon::ThreadPoolBuilder::new()
                    .build()
                    .expect("rayon thread pool must build");
                pool.install(|| {
                    loop {
                        let batch: Vec<String> = lines
                            .by_ref()
                            .take(BLOCKS_COUNT_PER_CHUNK)
                            .filter_map(Result::ok)
                            .collect();
                        if batch.is_empty() {
                            break;
                        }
                        batch.par_iter().for_each(|line| {
                            let block: JsonBlock =
                                serde_json::from_str(line).expect("parse block from json");
                            let block: Arc<core::BlockView> = Arc::new(block.into());
                            blocks_tx.send(block).expect("send block to channel");

                            #[cfg(feature = "progress_bar")]
                            progress_bar.inc(line.len() as u64);
                        });
                    }
                    drop(blocks_tx);
                });
            }
        });

        let callback = |verify_result: VerifyResult| {
            if let Err(err) = verify_result {
                eprintln!("Error verifying block: {:?}", err);
            }
        };

        for block in blocks_rx {
            if !block.is_genesis() {
                use ckb_chain::LonelyBlock;

                largest_block_number = largest_block_number.max(block.number());

                let lonely_block = LonelyBlock {
                    block,
                    switch: Some(self.switch),
                    verify_callback: Some(Box::new(callback)),
                };
                self.chain.asynchronous_process_lonely_block(lonely_block);
            }
        }

        while self
            .shared
            .snapshot()
            .get_block_hash(largest_block_number)
            .is_none()
        {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }

        #[cfg(feature = "progress_bar")]
        progress_bar.finish_with_message("done!");
        Ok(())
    }
}
