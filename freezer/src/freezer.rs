use crate::freezer_files::FreezerFiles;
use crate::internal_error;
use ckb_error::Error;
use ckb_types::{
    core::{BlockNumber, BlockView, HeaderView},
    packed,
    prelude::*,
};
use ckb_util::Mutex;
use fs2::FileExt;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

const LOCKNAME: &str = "FLOCK";

/// freeze result represent blkhash -> (blknum, txsnum) btree-map
/// sorted blkhash for making ranges for compaction
type FreezeResult = BTreeMap<packed::Byte32, (BlockNumber, u32)>;

struct Inner {
    pub(crate) files: FreezerFiles,
    pub(crate) tip: Option<HeaderView>,
}

/// Freezer is an memory mapped append-only database to store immutable chain data into flat files
#[derive(Clone)]
pub struct Freezer {
    inner: Arc<Mutex<Inner>>,
    number: Arc<AtomicU64>,
    /// file lock to prevent double opens
    pub(crate) lock: Arc<File>,
    /// stop flag
    pub stopped: Arc<AtomicBool>,
}

impl Freezer {
    /// creates a freezer at specified path
    pub fn open(path: PathBuf) -> Result<Freezer, Error> {
        let lock_path = path.join(LOCKNAME);
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .open(lock_path)
            .map_err(internal_error)?;
        lock.try_lock_exclusive().map_err(internal_error)?;
        let mut files = FreezerFiles::open(path).map_err(internal_error)?;
        let freezer_number = files.number();

        let mut tip = None;
        if freezer_number > 1 {
            let raw_block = files
                .retrieve(freezer_number - 1)
                .map_err(internal_error)?
                .ok_or_else(|| internal_error("freezer inconsistent"))?;
            let block = packed::BlockReader::from_slice(&raw_block)
                .map_err(internal_error)?
                .to_entity();
            tip = Some(block.header().into_view());
        }

        let inner = Inner { files, tip };
        Ok(Freezer {
            number: Arc::clone(&inner.files.number),
            inner: Arc::new(Mutex::new(inner)),
            lock: Arc::new(lock),
            stopped: Arc::new(AtomicBool::new(false)),
        })
    }

    /// freeze background process that periodically checks the chain data for any
    /// import progress and moves ancient data from the kv-db into the freezer.
    pub fn freeze<F>(
        &self,
        threshold: BlockNumber,
        get_block_by_number: F,
    ) -> Result<FreezeResult, Error>
    where
        F: Fn(BlockNumber) -> Option<BlockView>,
    {
        let number = self.number();
        let mut guard = self.inner.lock();
        let mut ret = BTreeMap::new();
        ckb_logger::trace!("freezer freeze start {} threshold {}", number, threshold);

        for number in number..threshold {
            if self.stopped.load(Ordering::SeqCst) {
                guard.files.sync_all().map_err(internal_error)?;
                return Ok(ret);
            }

            if let Some(block) = get_block_by_number(number) {
                if let Some(ref header) = guard.tip {
                    if header.hash() != block.header().parent_hash() {
                        return Err(internal_error(format!(
                            "appending unexpected block expected parent_hash {} have {}",
                            header.hash(),
                            block.header().parent_hash()
                        )));
                    }
                }
                let raw_block = block.data();
                guard
                    .files
                    .append(number, raw_block.as_slice())
                    .map_err(internal_error)?;

                ret.insert(
                    block.header().hash(),
                    (number, block.transactions().len() as u32),
                );
                guard.tip = Some(block.header());
                ckb_logger::trace!("freezer block append {}", number);
            } else {
                ckb_logger::error!("freezer block missing {}", number);
                break;
            }
        }
        guard.files.sync_all().map_err(internal_error)?;
        Ok(ret)
    }

    /// Retrieve an item with the given number
    pub fn retrieve(&self, number: BlockNumber) -> Result<Option<Vec<u8>>, Error> {
        self.inner
            .lock()
            .files
            .retrieve(number)
            .map_err(internal_error)
    }

    /// Return total item number in the freezer
    pub fn number(&self) -> BlockNumber {
        self.number.load(Ordering::SeqCst)
    }

    /// Truncate discards any recent data above the provided threshold number.
    pub fn truncate(&self, item: u64) -> Result<(), Error> {
        if item > 0 && ((item + 1) < self.number()) {
            let mut inner = self.inner.lock();
            inner.files.truncate(item).map_err(internal_error)?;

            let raw_block = inner
                .files
                .retrieve(item)
                .map_err(internal_error)?
                .expect("frozen number sync with files");
            let block = packed::BlockReader::from_slice(&raw_block)
                .map_err(internal_error)?
                .to_entity();
            inner.tip = Some(block.header().into_view());
        }
        Ok(())
    }
}
