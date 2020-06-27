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

struct Inner {
    pub(crate) files: FreezerFiles,
    pub(crate) frozen_tip: Option<HeaderView>,
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
        let frozen = files.number();

        let mut frozen_tip = None;
        if frozen > 1 {
            let raw_block = files.retrieve(frozen - 1)?;
            let block = packed::BlockReader::from_slice(&raw_block)
                .map_err(internal_error)?
                .to_entity();
            frozen_tip = Some(block.header().into_view());
        }

        let inner = Inner { files, frozen_tip };
        Ok(Freezer {
            number: Arc::clone(&inner.files.number),
            inner: Arc::new(Mutex::new(inner)),
            lock: Arc::new(lock),
        })
    }

    pub fn freeze<F>(&self, threshold: BlockNumber, get_block_by_number: F) -> Result<(), Error>
    where
        F: Fn(BlockNumber) -> Option<BlockView>,
    {
        let number = self.number();
        let mut guard = self.inner.lock();
        for number in number..threshold {
            if let Some(block) = get_block_by_number(number) {
                if let Some(ref header) = guard.frozen_tip {
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
                ckb_logger::debug!("freezer block append {}", number);
            } else {
                ckb_logger::error!("freezer block missing {}", number);
                break;
            }
        }
        guard.files.sync_all()
    }

    pub fn retrieve(&self, number: BlockNumber) -> Result<Vec<u8>, Error> {
        self.inner.lock().files.retrieve(number)
    }

    pub fn number(&self) -> BlockNumber {
        self.number.load(Ordering::SeqCst)
    }
}
