use crate::freezer_files::FreezerFiles;
use crate::internal_error;
use ckb_error::Error;
use ckb_types::{core::BlockNumber, packed, prelude::*};
use ckb_util::Mutex;
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const LOCKNAME: &str = "FLOCK";

struct Inner {
    pub(crate) files: Mutex<FreezerFiles>,
    pub(crate) lock: File,
}

#[derive(Clone)]
pub struct Freezer {
    inner: Arc<Inner>,
    number: Arc<AtomicU64>,
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
        let number = Arc::clone(&files.number);
        let inner = Inner {
            files: Mutex::new(files),
            lock,
        };
        Ok(Freezer {
            inner: Arc::new(inner),
            number,
        })
    }

    pub fn freeze<F>(&self, threshold: BlockNumber, callback: F) -> Result<(), Error>
    where
        F: Fn(BlockNumber) -> Option<packed::Block>,
    {
        let number = self.number();
        let mut guard = self.inner.files.lock();
        for number in number..threshold {
            if let Some(block) = callback(number) {
                guard.append(number, block.as_slice())?;
                ckb_logger::debug!("freezer block append {}", number);
            } else {
                ckb_logger::error!("freezer block missing {}", number);
                break;
            }
        }
        guard.sync_all()
    }

    pub fn retrieve(&self, number: BlockNumber) -> Result<Vec<u8>, Error> {
        self.inner.files.lock().retrieve(number)
    }

    pub fn number(&self) -> BlockNumber {
        self.number.load(Ordering::SeqCst)
    }
}
