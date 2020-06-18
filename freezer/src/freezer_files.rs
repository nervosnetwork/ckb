use crate::internal_error;
use ckb_error::Error;
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
const INDEX_ENTRY_SIZE: u64 = 12;

pub type FileId = u32;

pub(crate) struct Head {
    pub(crate) file: File,
    pub(crate) bytes: u64,
}

impl Head {
    pub fn new(file: File, bytes: u64) -> Self {
        Head { file, bytes }
    }

    pub fn write(&mut self, data: &[u8]) -> Result<(), Error> {
        self.file.write_all(data).map_err(internal_error)?;
        self.bytes = self.bytes + data.len() as u64;
        Ok(())
    }
}

pub struct FreezerFiles {
    pub files: BTreeMap<FileId, File>,
    pub(crate) tail_id: FileId,
    pub(crate) head: Head,
    pub number: Arc<AtomicU64>,
    pub max_size: u64,
    pub head_id: FileId,
    pub file_path: PathBuf,
    pub index: File,
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
        let mut bytes = Vec::with_capacity(INDEX_ENTRY_SIZE as usize);
        bytes.extend_from_slice(&self.file_id.to_be_bytes());
        bytes.extend_from_slice(&self.offset.to_be_bytes());
        bytes
    }

    pub fn decode(raw: &[u8]) -> Result<Self, Error> {
        debug_assert!(raw.len() == INDEX_ENTRY_SIZE as usize);
        let (raw_file_id, raw_offset) = raw.split_at(::std::mem::size_of::<u32>());
        let file_id = u32::from_be_bytes(raw_file_id.try_into().map_err(internal_error)?);
        let offset = u64::from_be_bytes(raw_offset.try_into().map_err(internal_error)?);
        Ok(IndexEntry { offset, file_id })
    }
}

impl FreezerFiles {
    pub fn open(file_path: PathBuf) -> Result<FreezerFiles, Error> {
        let mut files = FreezerFilesBuilder::new(file_path).build()?;
        files.preopen()?;
        Ok(files)
    }

    pub fn append(&mut self, number: u64, data: &[u8]) -> Result<(), Error> {
        if self.number.load(Ordering::SeqCst) != number {
            return Err(internal_error(""));
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

    pub(crate) fn read_all_index(&self) -> Result<Vec<u8>, Error> {
        let mut index = &self.index;
        let mut ret = Vec::new();
        index.seek(SeekFrom::Start(0)).map_err(internal_error)?;
        index.read_to_end(&mut ret).map_err(internal_error)?;
        Ok(ret)
    }

    pub fn sync_all(&self) -> Result<(), Error> {
        self.index.sync_all().map_err(internal_error)?;
        self.head.file.sync_all().map_err(internal_error)?;
        Ok(())
    }

    pub fn retrieve(&self, item: u64) -> Result<Vec<u8>, Error> {
        if item < 1 {
            return Err(internal_error("retrieve out of bounds"));
        }

        if self.number.load(Ordering::SeqCst) <= item {
            return Err(internal_error("retrieve out of bounds"));
        }

        let (start_offset, end_offset, file_id) = self.get_bounds(item)?;

        let mut file = self
            .files
            .get(&file_id)
            .ok_or_else(|| internal_error(format!("missing blk file {}", file_id)))?;

        let size = (end_offset - start_offset) as usize;
        let mut ret = Vec::with_capacity(size);
        ret.resize_with(size, Default::default);
        file.seek(SeekFrom::Start(start_offset))
            .map_err(internal_error)?;
        file.read_exact(&mut ret).map_err(internal_error)?;
        Ok(ret)
    }

    fn get_bounds(&self, item: u64) -> Result<(u64, u64, FileId), Error> {
        let mut buffer = [0; INDEX_ENTRY_SIZE as usize];
        let mut index = &self.index;
        index
            .seek(SeekFrom::Start(item * INDEX_ENTRY_SIZE))
            .map_err(internal_error)?;
        index.read_exact(&mut buffer).map_err(internal_error)?;
        let end_index = IndexEntry::decode(&buffer)?;
        if item == 1 {
            return Ok((0, end_index.offset, end_index.file_id));
        }

        index
            .seek(SeekFrom::Start((item - 1) * INDEX_ENTRY_SIZE))
            .map_err(internal_error)?;
        index.read_exact(&mut buffer).map_err(internal_error)?;
        let start_index = IndexEntry::decode(&buffer)?;

        if start_index.file_id != end_index.file_id {
            return Ok((0, end_index.offset, end_index.file_id));
        }

        Ok((start_index.offset, end_index.offset, end_index.file_id))
    }

    fn preopen(&mut self) -> Result<(), Error> {
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
        let index = IndexEntry { file_id, offset };
        self.index
            .write_all(&index.encode())
            .map_err(internal_error)?;
        Ok(())
    }

    fn release(&mut self, id: FileId) {
        self.files.remove(&id);
    }

    fn release_after(&mut self, id: FileId) {
        self.files.split_off(&id);
    }

    fn open_read_only(&mut self, id: FileId) -> Result<File, Error> {
        let mut opt = fs::OpenOptions::new();
        opt.read(true);
        self.open_file(id, opt)
    }

    fn open_truncated(&mut self, id: FileId) -> Result<File, Error> {
        let mut opt = fs::OpenOptions::new();
        opt.create(true).read(true).write(true).truncate(true);
        self.open_file(id, opt)
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

    pub(crate) fn max_file_size(mut self, size: u64) -> Self {
        self.max_file_size = size;
        self
    }

    pub fn build(self) -> Result<FreezerFiles, Error> {
        fs::create_dir_all(&self.file_path).map_err(internal_error)?;
        let mut index = self.open_index()?;

        let index_meta = index.metadata().map_err(internal_error)?;
        let mut index_size = index_meta.len();

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
        let mut head = self.open_append(self.file_path.join(head_file_name))?;

        let head_meta = head.metadata().map_err(internal_error)?;
        let mut head_size = head_meta.len();
        let expect_head_size = head_index.offset;

        while expect_head_size != head_size {
            if expect_head_size < head_size {
                ckb_logger::warn!(
                    "Truncating dangling head {} {}",
                    head_size,
                    expect_head_size,
                );
                helper::truncate_file(&mut head, expect_head_size)?;
            }

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
                if new_index.file_id != head_index.file_id {
                    let head_file_name = helper::file_name(head_index.file_id);
                    let new_head = self.open_append(self.file_path.join(head_file_name))?;
                    let new_head_meta = new_head.metadata().map_err(internal_error)?;

                    head_size = new_head_meta.len();
                    head = new_head;
                    head_index = new_index;
                }
            }
        }

        index.sync_all().map_err(internal_error)?;
        head.sync_all().map_err(internal_error)?;

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

    fn open_append<P: AsRef<Path>>(&self, path: P) -> Result<File, Error> {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)
            .map_err(internal_error)?;
        file.seek(SeekFrom::End(0)).map_err(internal_error)?;
        Ok(file)
    }

    fn open_index(&self) -> Result<File, Error> {
        let mut index = self.open_append(self.file_path.join(INDEX_FILE_NAME))?;
        let metadata = index.metadata().map_err(internal_error)?;
        println!("open_index {}", metadata.len());
        if metadata.len() == 0 {
            index
                .write_all(&IndexEntry::default().encode())
                .map_err(internal_error)?;
        }

        let tail = metadata.len() % INDEX_ENTRY_SIZE;
        if (tail != 0) && (metadata.len() != 0) {
            helper::truncate_file(&mut index, metadata.len() - tail)?;
        }
        Ok(index)
    }
}

mod helper {
    use super::*;

    pub(crate) fn truncate_file(file: &mut File, size: u64) -> Result<(), Error> {
        file.set_len(size).map_err(internal_error)?;
        file.seek(SeekFrom::End(0)).map_err(internal_error)?;
        Ok(())
    }

    pub(crate) fn file_name(file_id: FileId) -> String {
        format!("blk{:06}", file_id)
    }
}
