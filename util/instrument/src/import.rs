use ckb_app_config::ImportSource;
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
use std::sync::Arc;

/// Export block date from file to database.
pub struct Import {
    /// source file contains block data
    source: ImportSource,
    shared: Shared,
    chain: ChainController,
    switch: Switch,
    num_threads: usize,
}

impl Import {
    /// Creates a new import job.
    pub fn new(
        chain: ChainController,
        shared: Shared,
        source: ImportSource,
        switch: Switch,
        num_threads: usize,
    ) -> Self {
        Import {
            chain,
            shared,
            source,
            switch,
            num_threads,
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
    pub fn read_from_json(&self) -> Result<(), Box<dyn Error>> {
        use std::io::Read;

        use ckb_chain::VerifyResult;
        use ckb_types::core::BlockView;

        while self.chain.is_verifying_unverified_blocks_on_startup() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let f: Box<dyn Read + Send> = match &self.source {
            ImportSource::Path(source) => Box::new(fs::File::open(source)?),
            ImportSource::Stdin => {
                // read from stdin
                Box::new(std::io::stdin())
            }
        };

        let reader = io::BufReader::new(f);
        let mut lines = reader.lines();
        let first_line = lines.next().transpose()?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "The source file is empty.")
        })?;
        let first_block: JsonBlock = serde_json::from_str(&first_line).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse first block from json failed: {err}"),
            )
        })?;
        let first_block: core::BlockView = first_block.into();

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

                let source_display = match self.source {
                    ImportSource::Path(ref path) => path.display().to_string(),
                    ImportSource::Stdin => "stdin".to_string(),
                };

                return Err(Box::new(io::Error::other(format!(
                    "In {}, the first block is {}-{}, and its parent (hash: {}) was not found in the database. The current tip is {}-{}.",
                    source_display,
                    first_block.number(),
                    first_block.hash(),
                    first_block_parent,
                    tip.number(),
                    tip.hash(),
                ))));
            }
        }

        #[cfg(feature = "progress_bar")]
        let progress_bar = {
            let bar = match &self.source {
                ImportSource::Path(source) => {
                    let file_size = fs::metadata(source)?.len();
                    ProgressBar::new(file_size)
                }
                ImportSource::Stdin => ProgressBar::new_spinner(),
            };
            let style = ProgressStyle::default_bar()
                .template("[{elapsed_precise}] {bar:50.cyan/blue} {bytes:>6}/{total_bytes:6} {msg}")
                .expect("Failed to set progress bar template")
                .progress_chars("##-");
            bar.set_style(style);
            bar
        };

        const BLOCKS_COUNT_PER_CHUNK: usize = 1024 * 6;
        let (blocks_tx, blocks_rx) =
            ckb_channel::bounded::<(Arc<BlockView>, usize)>(BLOCKS_COUNT_PER_CHUNK);
        let parser_jh = std::thread::spawn({
            let num_threads = self.num_threads;
            move || -> Result<(), String> {
                let mut first_line = Some(first_line);
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(num_threads)
                    .build()
                    .map_err(|err| format!("rayon thread pool build failed: {err:?}"))?;
                pool.install(|| {
                    loop {
                        let mut batch = Vec::with_capacity(BLOCKS_COUNT_PER_CHUNK);
                        if let Some(line) = first_line.take() {
                            batch.push(line);
                        }
                        for line in lines.by_ref().take(BLOCKS_COUNT_PER_CHUNK - batch.len()) {
                            batch.push(
                                line.map_err(|err| format!("read jsonl line failed: {err}"))?,
                            );
                        }
                        if batch.is_empty() {
                            break;
                        }
                        batch
                            .par_iter()
                            .try_for_each(|line| -> Result<(), String> {
                                let block: JsonBlock =
                                    serde_json::from_str(line).map_err(|err| {
                                        format!("parse block from json failed: {err}")
                                    })?;
                                let block: Arc<core::BlockView> = Arc::new(block.into());
                                blocks_tx
                                    .send((block, line.len()))
                                    .map_err(|err| format!("send block to channel failed: {err:?}"))
                            })?;
                    }
                    drop(blocks_tx);
                    Ok(())
                })
            }
        });

        let (verify_tx, verify_rx) = ckb_channel::unbounded();
        let mut submitted_blocks = 0;
        for (block, block_size) in blocks_rx {
            if !block.is_genesis() {
                use ckb_chain::LonelyBlock;

                #[cfg(feature = "progress_bar")]
                let callback = {
                    let progress_bar = progress_bar.clone();
                    let verify_tx = verify_tx.clone();
                    let block_number = block.number();
                    Box::new(move |verify_result: VerifyResult| match verify_result {
                        Ok(true) => {
                            progress_bar.inc(block_size as u64);
                            let _ = verify_tx.send(Ok(block_number));
                        }
                        Ok(false) => {
                            let _ = verify_tx
                                .send(Err(format!("block {block_number} was not verified")));
                        }
                        Err(err) => {
                            eprintln!("Error verifying block: {:?}", err);
                            let _ = verify_tx
                                .send(Err(format!("verify block {block_number} failed: {err:?}")));
                        }
                    })
                };
                #[cfg(not(feature = "progress_bar"))]
                let callback = {
                    let _ = block_size;
                    let verify_tx = verify_tx.clone();
                    let block_number = block.number();
                    Box::new(move |verify_result: VerifyResult| match verify_result {
                        Ok(true) => {
                            let _ = verify_tx.send(Ok(block_number));
                        }
                        Ok(false) => {
                            let _ = verify_tx
                                .send(Err(format!("block {block_number} was not verified")));
                        }
                        Err(err) => {
                            eprintln!("Error verifying block: {:?}", err);
                            let _ = verify_tx
                                .send(Err(format!("verify block {block_number} failed: {err:?}")));
                        }
                    })
                };

                let lonely_block = LonelyBlock {
                    block,
                    switch: Some(self.switch),
                    verify_callback: Some(callback),
                };
                self.chain.asynchronous_process_lonely_block(lonely_block);
                submitted_blocks += 1;
            }
        }

        match parser_jh.join() {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                return Err(Box::new(io::Error::new(io::ErrorKind::InvalidData, err)));
            }
            Err(_) => {
                return Err(Box::new(io::Error::other("jsonl parser thread panicked")));
            }
        }

        drop(verify_tx);
        for _ in 0..submitted_blocks {
            match verify_rx.recv() {
                Ok(Ok(_block_number)) => {}
                Ok(Err(err)) => return Err(Box::new(io::Error::other(err))),
                Err(err) => {
                    return Err(Box::new(io::Error::other(format!(
                        "verify result channel closed unexpectedly: {err}"
                    ))));
                }
            }
        }

        #[cfg(feature = "progress_bar")]
        progress_bar.finish_with_message("done!");
        Ok(())
    }
}
