use crate::freezer_files::FreezerFiles;
use crate::internal_error;
use crate::reader::FreezerReader;
use crate::{ArchiveRecord, ArchivedBlock};
use ckb_error::Error;
use ckb_types::{packed, prelude::*};
use ckb_util::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// An append-only archive with one writer and independent committed readers.
#[derive(Clone)]
pub struct Freezer {
    inner: Arc<Mutex<FreezerFiles>>,
    pub(crate) reader: Arc<FreezerReader>,
    /// stop flag
    pub stopped: Arc<AtomicBool>,
}

impl Freezer {
    /// Open an archive and validate its latest committed block's layout.
    pub fn open(path: PathBuf) -> Result<Freezer, Error> {
        let files = FreezerFiles::open(path).map_err(internal_error)?;
        let freezer_number = files.committed_number();

        // Opening validated every committed index entry. Historical payloads
        // are checked on read, so startup need not decode the entire archive.
        if freezer_number > 1 {
            let raw_block = files
                .retrieve(freezer_number - 1)
                .map_err(internal_error)?
                .ok_or_else(|| internal_error("freezer inconsistent"))?;
            let block =
                packed::BlockReader::from_compatible_slice(&raw_block).map_err(internal_error)?;
            if block.count_extra_fields() > 1 {
                return Err(internal_error("block has more than one extra fields"));
            }
        }

        Ok(Freezer {
            reader: Arc::clone(&files.reader),
            inner: Arc::new(Mutex::new(files)),
            stopped: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Open an archive at the supplied path.
    pub fn open_in<P: AsRef<Path>>(path: P) -> Result<Freezer, Error> {
        Self::open(path.as_ref().to_path_buf())
    }

    /// Append immutable blocks and commit their file records in input order.
    /// Record numbers are independent of block heights and canonical chain order.
    pub(crate) fn append_blocks(
        &self,
        blocks: &[packed::Block],
    ) -> Result<std::ops::Range<u64>, Error> {
        if blocks.iter().any(|block| block.count_extra_fields() > 1) {
            return Err(internal_error("block has more than one extra field"));
        }
        let mut guard = self.inner.lock();
        let start = guard.number();
        for block in blocks {
            let number = guard.number();
            guard
                .append(number, block.as_slice())
                .map_err(internal_error)?;
        }
        guard.sync_all().map_err(internal_error)?;
        Ok(start..guard.number())
    }

    pub(crate) fn sync(&self) -> Result<(), Error> {
        self.inner.lock().sync_all().map_err(internal_error)
    }

    #[cfg(test)]
    pub(crate) fn with_writer<T>(&self, f: impl FnOnce(&mut FreezerFiles) -> T) -> T {
        f(&mut self.inner.lock())
    }

    /// Retrieve an item with the given number
    pub fn retrieve(&self, number: u64) -> Result<Option<Vec<u8>>, Error> {
        self.reader.retrieve(number).map_err(internal_error)
    }

    /// Prepare immutable metadata once, so a budget check and its subsequent
    /// read use the same validated index entry.
    pub fn record(&self, number: u64) -> Result<Option<ArchiveRecord<'_>>, Error> {
        self.reader.record(number).map_err(internal_error)
    }

    /// Validate a block's layout and exact header hash for selective field reads.
    pub fn read_block(
        &self,
        number: u64,
        hash: &packed::Byte32,
    ) -> Result<Option<ArchivedBlock<'_>>, Error> {
        self.record(number)?
            .map(|record| ArchivedBlock::new(record, hash))
            .transpose()
            .map_err(internal_error)
    }

    /// Reject a read exceeding the buffer budget before allocating either buffer.
    pub fn retrieve_limited(
        &self,
        number: u64,
        max_bytes: usize,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.reader
            .retrieve_limited(number, max_bytes)
            .map_err(internal_error)
    }

    /// The next item number after the durable, publicly readable prefix.
    pub fn number(&self) -> u64 {
        self.reader.number()
    }
}
