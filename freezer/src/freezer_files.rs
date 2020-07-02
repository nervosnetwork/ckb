use crate::internal_error;
use ckb_error::Error;
use fail::fail_point;
use std::collections::BTreeMap;
use std::convert::TryInto;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::io::{Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const MAX_FILE_SIZE: u64 = 2 * 1_000 * 1_000 * 1_000;
const INDEX_FILE_NAME: &str = "INDEX";
pub(crate) const INDEX_ENTRY_SIZE: u64 = 12;

pub type FileId = u32;

pub(crate) struct Head {
    pub(crate) file: File,
    // number of bytes written to the head file
    pub(crate) bytes: u64,
}

impl Head {
    pub fn new(file: File, bytes: u64) -> Self {
        Head { file, bytes }
    }

    pub fn write(&mut self, data: &[u8]) -> Result<(), Error> {
        fail_point!("write-head");
        self.file.write_all(data).map_err(internal_error)?;
        self.bytes += data.len() as u64;
        Ok(())
    }
}

// FreezerFiles represents a single chained block data,
// it consists of a data file and an index file
pub struct FreezerFiles {
    // opened files
    pub(crate) files: BTreeMap<FileId, File>,
    // head file
    pub(crate) head: Head,
    // number of frozen
    pub(crate) number: Arc<AtomicU64>,
    // max size for data-files
    max_size: u64,
    // number of the earliest file
    pub(crate) tail_id: FileId,
    // number of the currently active head file
    pub(crate) head_id: FileId,
    // data file path
    file_path: PathBuf,
    // index for freezer files
    pub(crate) index: File,
}

pub struct IndexEntry {
    pub file_id: FileId,
    pub offset: u64,
}

impl Default for IndexEntry {
    fn default() -> Self {
        IndexEntry {
            file_id: 0,
            offset: 0,
        }
    }
}

impl IndexEntry {
    pub fn encode(&self) -> Vec<u8> {
        fail_point!("IndexEntry encode");
        let mut bytes = Vec::with_capacity(INDEX_ENTRY_SIZE as usize);
        bytes.extend_from_slice(&self.file_id.to_le_bytes());
        bytes.extend_from_slice(&self.offset.to_le_bytes());
        bytes
    }

    pub fn decode(raw: &[u8]) -> Result<Self, Error> {
        fail_point!("IndexEntry decode");
        debug_assert!(raw.len() == INDEX_ENTRY_SIZE as usize);
        let (raw_file_id, raw_offset) = raw.split_at(::std::mem::size_of::<u32>());
        let file_id = u32::from_le_bytes(raw_file_id.try_into().map_err(internal_error)?);
        let offset = u64::from_le_bytes(raw_offset.try_into().map_err(internal_error)?);
        Ok(IndexEntry { offset, file_id })
    }
}

impl FreezerFiles {
    pub fn open(file_path: PathBuf) -> Result<FreezerFiles, Error> {
        let mut files = FreezerFilesBuilder::new(file_path).build()?;
        files.preopen()?;
        Ok(files)
    }

    #[inline]
    pub fn number(&self) -> u64 {
        self.number.load(Ordering::SeqCst)
    }

    pub fn append(&mut self, number: u64, data: &[u8]) -> Result<(), Error> {
        let expected = self.number.load(Ordering::SeqCst);
        fail_point!("append-unexpected-number");
        if expected != number {
            return Err(internal_error(format!(
                "appending unexpected block expected {} have {}",
                expected, number
            )));
        }

        let data_size = data.len();
        // open a new file
        if self.head.bytes + data_size as u64 > self.max_size {
            let head_id = self.head_id;
            let next_id = head_id + 1;
            let new_head_file = self.open_truncated(next_id)?;

            // release old head, reopen with read only
            self.release(head_id);
            self.open_read_only(head_id)?;

            self.head_id = next_id;
            self.head = Head::new(new_head_file, 0);
        }

        self.head.write(data)?;
        self.write_index(self.head_id, self.head.bytes)?;
        self.number.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    pub fn sync_all(&self) -> Result<(), Error> {
        self.head.file.sync_all().map_err(internal_error)?;
        self.index.sync_all().map_err(internal_error)?;
        Ok(())
    }

    pub fn retrieve(&self, item: u64) -> Result<Option<Vec<u8>>, Error> {
        if item < 1 {
            return Ok(None);
        }
        if self.number.load(Ordering::SeqCst) <= item {
            return Ok(None);
        }

        let bounds = self.get_bounds(item)?;
        if let Some((start_offset, end_offset, file_id)) = bounds {
            let mut file = self
                .files
                .get(&file_id)
                .ok_or_else(|| internal_error(format!("missing blk file {}", file_id)))?;

            let size = (end_offset - start_offset) as usize;
            let mut ret = vec![0u8; size];
            file.seek(SeekFrom::Start(start_offset))
                .map_err(internal_error)?;
            file.read_exact(&mut ret).map_err(internal_error)?;

            Ok(Some(ret))
        } else {
            Ok(None)
        }
    }

    fn get_bounds(&self, item: u64) -> Result<Option<(u64, u64, FileId)>, Error> {
        let mut buffer = [0; INDEX_ENTRY_SIZE as usize];
        let mut index = &self.index;
        if let Err(e) = index.seek(SeekFrom::Start(item * INDEX_ENTRY_SIZE)) {
            ckb_logger::trace!("Freezer get_bounds seek {} {}", item * INDEX_ENTRY_SIZE, e);
            return Ok(None);
        }

        if let Err(e) = index.read_exact(&mut buffer) {
            ckb_logger::trace!("Freezer get_bounds read_exact {}", e);
            return Ok(None);
        }
        let end_index = IndexEntry::decode(&buffer)?;
        if item == 1 {
            return Ok(Some((0, end_index.offset, end_index.file_id)));
        }

        if let Err(e) = index.seek(SeekFrom::Start((item - 1) * INDEX_ENTRY_SIZE)) {
            ckb_logger::trace!(
                "Freezer get_bounds seek {} {}",
                (item - 1) * INDEX_ENTRY_SIZE,
                e
            );
            return Ok(None);
        }
        if let Err(e) = index.read_exact(&mut buffer) {
            ckb_logger::trace!("Freezer get_bounds read_exact {}", e);
            return Ok(None);
        }
        let start_index = IndexEntry::decode(&buffer)?;
        if start_index.file_id != end_index.file_id {
            return Ok(Some((0, end_index.offset, end_index.file_id)));
        }

        Ok(Some((
            start_index.offset,
            end_index.offset,
            end_index.file_id,
        )))
    }

    // keeping the the provided threshold number item and dropping the rest.
    pub fn truncate(&mut self, item: u64) -> Result<(), Error> {
        // out of bound, this has no effect.
        if item < 1 || ((item + 1) >= self.number()) {
            return Ok(());
        }
        ckb_logger::trace!("Freezer truncate items {}", item);

        let mut buffer = [0; INDEX_ENTRY_SIZE as usize];
        // truncate the index
        helper::truncate_file(&mut self.index, (item + 1) * INDEX_ENTRY_SIZE)?;
        self.index
            .seek(SeekFrom::Start(item * INDEX_ENTRY_SIZE))
            .map_err(internal_error)?;
        self.index.read_exact(&mut buffer).map_err(internal_error)?;
        let new_index = IndexEntry::decode(&buffer)?;

        // truncate files
        if new_index.file_id != self.head_id {
            self.release(new_index.file_id);
            let (new_head_file, offset) = self.open_append(new_index.file_id)?;

            self.delete_after(new_index.file_id)?;

            self.head_id = new_index.file_id;
            self.head = Head::new(new_head_file, offset);
        }
        helper::truncate_file(&mut self.head.file, new_index.offset)?;
        self.head.bytes = new_index.offset;
        self.number.store(item + 1, Ordering::SeqCst);
        Ok(())
    }

    pub fn preopen(&mut self) -> Result<(), Error> {
        self.release_after(0);

        for id in self.tail_id..self.head_id {
            self.open_read_only(id)?;
        }
        self.files.insert(
            self.head_id,
            self.head.file.try_clone().map_err(internal_error)?,
        );
        Ok(())
    }

    fn write_index(&mut self, file_id: FileId, offset: u64) -> Result<(), Error> {
        fail_point!("write-index");
        let index = IndexEntry { file_id, offset };
        self.index.seek(SeekFrom::End(0)).map_err(internal_error)?;
        self.index
            .write_all(&index.encode())
            .map_err(internal_error)?;
        Ok(())
    }

    fn release(&mut self, id: FileId) {
        self.files.remove(&id);
    }

    fn release_after(&mut self, id: FileId) {
        self.files.split_off(&(id + 1));
    }

    fn delete_after(&mut self, id: FileId) -> Result<(), Error> {
        let released = self.files.split_off(&(id + 1));
        self.delete_files_by_id(released.keys().cloned())
    }

    fn delete_files_by_id(&self, file_ids: impl Iterator<Item = FileId>) -> Result<(), Error> {
        for file_id in file_ids {
            let path = self.file_path.join(helper::file_name(file_id));
            fs::remove_file(path).map_err(internal_error)?;
        }
        Ok(())
    }

    fn open_read_only(&mut self, id: FileId) -> Result<File, Error> {
        fail_point!("open_read_only");
        let mut opt = fs::OpenOptions::new();
        opt.read(true);
        self.open_file(id, opt)
    }

    fn open_truncated(&mut self, id: FileId) -> Result<File, Error> {
        fail_point!("open_truncated");
        let mut opt = fs::OpenOptions::new();
        opt.create(true).read(true).write(true).truncate(true);
        self.open_file(id, opt)
    }

    fn open_append(&mut self, id: FileId) -> Result<(File, u64), Error> {
        fail_point!("open_append");
        let mut opt = fs::OpenOptions::new();
        opt.create(true).read(true).write(true);
        let mut file = self.open_file(id, opt)?;
        let offset = file.seek(SeekFrom::End(0)).map_err(internal_error)?;
        Ok((file, offset))
    }

    fn open_file(&mut self, id: FileId, opt: fs::OpenOptions) -> Result<File, Error> {
        let name = helper::file_name(id);
        let file = opt
            .open(self.file_path.join(name))
            .map_err(internal_error)?;
        self.files
            .insert(id, file.try_clone().map_err(internal_error)?);
        Ok(file)
    }
}

pub struct FreezerFilesBuilder {
    file_path: PathBuf,
    max_file_size: u64,
}

impl FreezerFilesBuilder {
    pub fn new(file_path: PathBuf) -> Self {
        FreezerFilesBuilder {
            file_path,
            max_file_size: MAX_FILE_SIZE,
        }
    }

    // for test
    #[allow(dead_code)]
    pub fn max_file_size(mut self, size: u64) -> Self {
        self.max_file_size = size;
        self
    }

    pub fn build(self) -> Result<FreezerFiles, Error> {
        fs::create_dir_all(&self.file_path).map_err(internal_error)?;
        let (mut index, mut index_size) = self.open_index()?;

        let mut buffer = [0; INDEX_ENTRY_SIZE as usize];
        index.seek(SeekFrom::Start(0)).map_err(internal_error)?;
        index.read_exact(&mut buffer).map_err(internal_error)?;
        let tail_index = IndexEntry::decode(&buffer)?;
        let tail_id = tail_index.file_id;

        index
            .seek(SeekFrom::Start(index_size - INDEX_ENTRY_SIZE))
            .map_err(internal_error)?;
        index.read_exact(&mut buffer).map_err(internal_error)?;

        ckb_logger::debug!("Freezer index_size {} head {:?}", index_size, buffer);

        let mut head_index = IndexEntry::decode(&buffer)?;
        let head_file_name = helper::file_name(head_index.file_id);
        let (mut head, mut head_size) = self.open_append(self.file_path.join(head_file_name))?;
        let mut expect_head_size = head_index.offset;

        // try repair cross checks the head and the index file and truncates them to
        // be in sync with each other after a potential crash/data loss.
        while expect_head_size != head_size {
            // truncate the head file to the last offset
            if expect_head_size < head_size {
                ckb_logger::warn!(
                    "Truncating dangling head {} {}",
                    head_size,
                    expect_head_size,
                );
                helper::truncate_file(&mut head, expect_head_size)?;
                head_size = expect_head_size;
            }

            // truncate the index to matching the head file
            if expect_head_size > head_size {
                ckb_logger::warn!(
                    "Truncating dangling indexes {} {}",
                    head_size,
                    expect_head_size,
                );
                helper::truncate_file(&mut index, index_size - INDEX_ENTRY_SIZE)?;
                index_size -= INDEX_ENTRY_SIZE;

                index
                    .seek(SeekFrom::Start(index_size - INDEX_ENTRY_SIZE))
                    .map_err(internal_error)?;
                index.read_exact(&mut buffer).map_err(internal_error)?;
                let new_index = IndexEntry::decode(&buffer)?;

                // slipped back into an earlier head-file
                if new_index.file_id != head_index.file_id {
                    let head_file_name = helper::file_name(head_index.file_id);
                    let (new_head, size) = self.open_append(self.file_path.join(head_file_name))?;
                    head = new_head;
                    head_size = size;
                }
                expect_head_size = new_index.offset;
                head_index = new_index;
            }
        }

        // ensure flush to disk
        head.sync_all().map_err(internal_error)?;
        index.sync_all().map_err(internal_error)?;

        let number = index_size / INDEX_ENTRY_SIZE;

        Ok(FreezerFiles {
            files: BTreeMap::new(),
            head: Head::new(head, head_size),
            tail_id,
            number: Arc::new(AtomicU64::new(number)),
            max_size: self.max_file_size,
            head_id: head_index.file_id,
            file_path: self.file_path,
            index,
        })
    }

    // Open the file without append mode
    // If a file is opened with both read and append access,
    // after opening, and after every write,
    // the position for reading may be set at the end of the file.
    // it has differing behaviour on different OS
    fn open_append<P: AsRef<Path>>(&self, path: P) -> Result<(File, u64), Error> {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)
            .map_err(internal_error)?;
        let offset = file.seek(SeekFrom::End(0)).map_err(internal_error)?;
        Ok((file, offset))
    }

    fn open_index(&self) -> Result<(File, u64), Error> {
        let (mut index, mut size) = self.open_append(self.file_path.join(INDEX_FILE_NAME))?;
        // fill a default entry within empty index
        if size == 0 {
            index
                .write_all(&IndexEntry::default().encode())
                .map_err(internal_error)?;
            size += INDEX_ENTRY_SIZE;
        }

        // ensure the index is a multiple of INDEX_ENTRY_SIZE bytes
        let tail = size % INDEX_ENTRY_SIZE;
        if (tail != 0) && (size != 0) {
            size -= tail;
            helper::truncate_file(&mut index, size)?;
        }
        Ok((index, size))
    }
}

pub(crate) mod helper {
    use super::*;

    pub(crate) fn truncate_file(file: &mut File, size: u64) -> Result<(), Error> {
        file.set_len(size).map_err(internal_error)?;
        file.seek(SeekFrom::End(0)).map_err(internal_error)?;
        Ok(())
    }

    #[inline]
    pub(crate) fn file_name(file_id: FileId) -> String {
        format!("blk{:06}", file_id)
    }
}
