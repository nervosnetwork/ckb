use ckb_jsonrpc_types::BlockView as JsonBlock;
use ckb_jsonrpc_types::Either;
use ckb_shared::Snapshot;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_types::H256;
use ckb_types::core::BlockNumber;
#[cfg(feature = "progress_bar")]
use indicatif::{ParallelProgressIterator, ProgressBar, ProgressStyle};
use itertools::Itertools;
use rayon::prelude::*;
use std::error::Error;
use std::fs;
use std::io;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

/// Export block from database to specify file.
pub struct Export {
    /// export target path
    pub target: PathBuf,
    /// CKB shared data.
    pub shared: Shared,
    /// the snapshot of the shared data
    pub snapshot: Arc<Snapshot>,

    /// The range start block number or block hash.
    pub from: Option<Either<BlockNumber, H256>>,
    /// The range end block number or block hash.
    pub to: Option<Either<BlockNumber, H256>>,
}

impl Export {
    /// Creates the export job.
    pub fn new(
        shared: Shared,
        target: PathBuf,
        from: Option<Either<BlockNumber, H256>>,
        to: Option<Either<BlockNumber, H256>>,
    ) -> Self {
        let snapshot = shared.cloned_snapshot();
        Export {
            shared,
            snapshot,
            target,
            from,
            to,
        }
    }

    /// export file name
    fn file_name(&self, from_number: u64, to_number: u64) -> Result<String, Box<dyn Error>> {
        Ok(format!(
            "{}-{}-{}.{}",
            self.shared.consensus().id,
            from_number,
            to_number,
            "jsonl"
        ))
    }

    /// Executes the export job.
    pub fn execute(self) -> Result<(), Box<dyn Error>> {
        fs::create_dir_all(&self.target)?;
        self.write_to_json()
    }

    /// export ckb data to a jsonl file
    pub fn write_to_json(self) -> Result<(), Box<dyn Error>> {
        let from_number = match self.from.clone() {
            Some(Either::Left(number)) => number,
            Some(Either::Right(hash)) => self
                .snapshot
                .get_block_number(&hash.clone().into())
                .unwrap_or_else(|| panic!("must get block number for hash: {}", hash)),
            None => 0,
        };
        let to_number = match self.to.clone() {
            Some(Either::Left(number)) => number,
            Some(Either::Right(hash)) => self
                .snapshot
                .get_block_number(&hash.clone().into())
                .unwrap_or_else(|| panic!("must get block number for hash: {}", hash)),
            None => self.snapshot.tip_number(),
        };

        if to_number < from_number {
            return Err(format!("Invalid range: from {} to {}", from_number, to_number).into());
        }

        let file_name = self.file_name(from_number, to_number)?;
        let file_path = self.target.join(file_name);
        println!("Writing to {}", file_path.display());
        let f = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(file_path)?;

        let mut writer = io::BufWriter::new(f);
        let snapshot = self.shared.snapshot();

        #[cfg(feature = "progress_bar")]
        let progress_bar = {
            let progress_bar = ProgressBar::new(to_number - from_number + 1);
            progress_bar.set_style(
                ProgressStyle::default_bar()
                    .template("[{elapsed_precise}] {bar:50.cyan/blue} {pos:>6}/{len:6} {msg}")
                    .expect("Failed to set progress bar template")
                    .progress_chars("##-"),
            );
            progress_bar
        };

        const BLOCKS_COUNT_PER_CHUNK: usize = 1024;
        let (blocks_tx, blocks_rx) = ckb_channel::bounded(BLOCKS_COUNT_PER_CHUNK);

        std::thread::scope({
            |s| {
                #[cfg(feature = "progress_bar")]
                let progress_bar_clone = progress_bar.clone();
                let producer_jh = s.spawn(move || -> Result<(), String> {
                    (from_number..=to_number)
                        .chunks(BLOCKS_COUNT_PER_CHUNK)
                        .into_iter()
                        .try_for_each(|chunk| -> Result<(), String> {
                            chunk
                                .collect::<Vec<_>>()
                                .into_par_iter()
                                .progress_with(progress_bar_clone.clone())
                                .try_for_each(|block_number| -> Result<(), String> {
                                    let block_hash =
                                        snapshot.get_block_hash(block_number).ok_or_else(|| {
                                            format!("not found block hash for {}", block_number)
                                        })?;
                                    let block =
                                        snapshot.get_block(&block_hash).ok_or_else(|| {
                                            format!("not found block for {}", block_number)
                                        })?;
                                    let block: JsonBlock = block.into();
                                    let encoded = serde_json::to_vec(&block).map_err(|err| {
                                        format!("serializing block failed: {:?}", err)
                                    })?;
                                    blocks_tx
                                        .send((block_number, encoded))
                                        .map_err(|err| format!("sending block failed: {:?}", err))
                                })
                        })?;

                    drop(blocks_tx);
                    Ok(())
                });
                let mut expected = from_number;

                let mut buffer = std::collections::BTreeMap::new();
                for (idx, res) in blocks_rx {
                    buffer.insert(idx, res);
                    while buffer.contains_key(&expected) {
                        let encoded = buffer.remove(&expected).unwrap();
                        if let Err(err) = writer.write_all(&encoded) {
                            eprintln!("writing block failed: {:?}", err);
                        }
                        if let Err(err) = writer.write_all(b"\n") {
                            eprintln!("writing newline failed: {:?}", err);
                        }
                        expected += 1;
                    }
                }

                let result = producer_jh.join();
                if let Err(err) = result {
                    eprintln!("Error during export: {:?}", err);
                    return;
                }

                #[cfg(feature = "progress_bar")]
                progress_bar.finish_with_message("done!");
            }
        });
        Ok(())
    }
}
