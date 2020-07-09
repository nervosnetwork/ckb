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
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const LOCKNAME: &str = "FLOCK";
type FreezeResult = (BlockNumber, packed::Byte32, u32);

struct Inner {
    pub(crate) files: FreezerFiles,
    pub(crate) tip: Option<HeaderView>,
}

#[derive(Clone)]
pub struct Freezer {
    inner: Arc<Mutex<Inner>>,
    number: Arc<AtomicU64>,
    pub(crate) lock: Arc<File>,
}

impl Freezer {
    pub fn open(path: PathBuf) -> Result<Freezer, Error> {
        let lock_path = path.join(LOCKNAME);
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .open(lock_path)
            .map_err(internal_error)?;
        lock.try_lock_exclusive().map_err(internal_error)?;
        let files = FreezerFiles::open(path)?;
        let freezer_number = files.number();

        let mut tip = None;
        if freezer_number > 1 {
            let raw_block = files
                .retrieve(freezer_number - 1)?
                .expect("freezer number sync with files");
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
        })
    }

    pub fn freeze<F>(
        &self,
        threshold: BlockNumber,
        get_block_by_number: F,
    ) -> Result<Vec<FreezeResult>, Error>
    where
        F: Fn(BlockNumber) -> Option<BlockView>,
    {
        let number = self.number();
        let mut guard = self.inner.lock();
        let mut ret = Vec::with_capacity(threshold.saturating_sub(number) as usize);
        ckb_logger::info!("freezer freeze start {} threshold {}", number, threshold);

        for number in number..threshold {
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
                guard.files.append(number, raw_block.as_slice())?;
                ret.push((
                    number,
                    block.header().hash(),
                    block.transactions().len() as u32,
                ));
                guard.tip = Some(block.header());
                ckb_logger::info!("freezer block append {}", number);
            } else {
                ckb_logger::error!("freezer block missing {}", number);
                break;
            }
        }
        guard.files.sync_all()?;
        Ok(ret)
    }

    pub fn retrieve(&self, number: BlockNumber) -> Result<Option<Vec<u8>>, Error> {
        self.inner.lock().files.retrieve(number)
    }

    pub fn number(&self) -> BlockNumber {
        self.number.load(Ordering::SeqCst)
    }

    pub fn truncate(&self, item: u64) -> Result<(), Error> {
        if item > 0 && ((item + 1) < self.number()) {
            let mut inner = self.inner.lock();
            inner.files.truncate(item)?;

            let raw_block = inner
                .files
                .retrieve(item)?
                .expect("frozen number sync with files");
            let block = packed::BlockReader::from_slice(&raw_block)
                .map_err(internal_error)?
                .to_entity();
            inner.tip = Some(block.header().into_view());
        }
        Ok(())
    }
}
