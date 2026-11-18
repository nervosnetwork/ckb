use crate::decode_cache::DecodeCache;
use crate::format::{INDEX_ENTRY_SIZE, IndexEntry};
use crate::payload::{self, CHUNK_SIZE, ChunkTable, buffer};
use crate::storage::{data_path, invalid_data, read_exact_at};
use ckb_error::Error;
use ckb_util::Mutex;
use lru::LruCache;
use std::borrow::Cow;
use std::fs::File;
use std::io;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Reads immutable, committed records independently of the archive writer.
pub struct FreezerReader {
    pub(crate) path: PathBuf,
    index: File,
    files: Mutex<LruCache<u32, Arc<File>>>,
    decoded: DecodeCache,
    count: AtomicU64,
    // Writer-owned readers retain its lock, even if they outlive the writer.
    // Independent readers see only a committed prefix and never repair files.
    _lock: Option<File>,
}

impl FreezerReader {
    pub(crate) fn new(
        path: PathBuf,
        index: File,
        count: u64,
        limit: usize,
        lock: Option<File>,
    ) -> Self {
        Self {
            path,
            index,
            files: Mutex::new(LruCache::new(limit)),
            decoded: DecodeCache::new(),
            count: AtomicU64::new(count),
            _lock: lock,
        }
    }

    /// Open an independent reader without taking the writer lock or repairing files.
    /// The directory and its committed prefix must remain the same archive while open.
    pub fn open(path: &std::path::Path) -> io::Result<Self> {
        let index = File::open(path.join("INDEX"))?;
        let commit = crate::storage::read_commit(path)?;
        let mut tail = [0; INDEX_ENTRY_SIZE as usize];
        if let Some(last) = commit.count.checked_sub(1) {
            let offset = last
                .checked_mul(INDEX_ENTRY_SIZE)
                .ok_or_else(|| invalid_data("archive index length overflow"))?;
            read_exact_at(&index, &mut tail, offset)?;
            let entry = IndexEntry::decode(&tail)?;
            if entry.number != commit.count
                || entry.stored_len == 0
                || entry.file_id != commit.file_id
                || entry.offset.checked_add(u64::from(entry.stored_len)) != Some(commit.offset)
                || crate::format::checksum(&tail) != commit.index_hash
            {
                return Err(invalid_data("archive commit does not match its index"));
            }
            if std::fs::metadata(data_path(path, commit.file_id))?.len() < commit.offset {
                return Err(invalid_data("committed archive data is truncated"));
            }
        } else if commit != Default::default() {
            return Err(invalid_data("invalid empty archive commit"));
        }
        Ok(Self::new(
            path.to_owned(),
            index,
            commit.count,
            crate::freezer_files::OPEN_FILES_LIMIT,
            None,
        ))
    }

    /// Validate a record's block layout and the expected header hash.
    pub fn read_block(
        &self,
        number: u64,
        hash: &ckb_types::packed::Byte32,
    ) -> Result<Option<crate::ArchivedBlock<'_>>, Error> {
        self.record(number)
            .and_then(|record| {
                record
                    .map(|record| crate::ArchivedBlock::new(record, hash))
                    .transpose()
            })
            .map_err(crate::internal_error)
    }

    /// First record outside the committed prefix (record zero is reserved).
    pub fn number(&self) -> u64 {
        self.count.load(Ordering::Acquire) + 1
    }

    pub(crate) fn publish(&self, count: u64) {
        self.count.store(count, Ordering::Release);
    }

    /// Read a complete, checksummed record from this prefix.
    pub fn retrieve(&self, number: u64) -> io::Result<Option<Vec<u8>>> {
        self.retrieve_limited(number, usize::MAX)
    }

    pub(crate) fn record(&self, number: u64) -> io::Result<Option<ArchiveRecord<'_>>> {
        if number == 0 || number >= self.number() {
            return Ok(None);
        }
        let mut raw = [0; INDEX_ENTRY_SIZE as usize];
        read_exact_at(&self.index, &mut raw, (number - 1) * INDEX_ENTRY_SIZE)?;
        let entry = IndexEntry::decode(&raw)?;
        if entry.number != number || entry.stored_len == 0 {
            return Err(invalid_data(format!("invalid archive index item {number}")));
        }
        let raw_len = usize::try_from(entry.raw_len).map_err(invalid_data)?;
        if entry.stored_len as usize > payload::encoded_size_bound(raw_len)? {
            return Err(invalid_data("invalid archive compressed record length"));
        }
        let table_len = payload::table_len(raw_len)?;
        if table_len != 0 && table_len >= entry.stored_len as usize {
            return Err(invalid_data("invalid archive chunk table length"));
        }
        let read_bytes = raw_len
            .checked_add(entry.stored_len as usize)
            .ok_or_else(|| invalid_data("archive read size overflow"))?;
        entry
            .offset
            .checked_add(u64::from(entry.stored_len))
            .ok_or_else(|| invalid_data("archive file offset overflow"))?;
        count_read(INDEX_ENTRY_SIZE);
        Ok(Some(ArchiveRecord {
            reader: self,
            entry,
            raw_len,
            read_bytes,
        }))
    }

    /// Bound the combined compressed and decoded buffers before allocating.
    /// Read and verify a record within the caller's I/O budget.
    pub fn retrieve_limited(&self, number: u64, max_bytes: usize) -> io::Result<Option<Vec<u8>>> {
        self.record(number)?
            .map(|record| record.read(max_bytes))
            .transpose()
    }

    fn file(&self, id: u32) -> io::Result<Arc<File>> {
        {
            let mut files = self.files.lock();
            if let Some(file) = files.get(&id) {
                return Ok(Arc::clone(file));
            }
        }
        // Opening and reading files must not hold the cache lock. Concurrent
        // misses may open twice; only one handle is retained by the cache.
        let file = Arc::new(File::open(data_path(&self.path, id))?);
        let mut files = self.files.lock();
        if let Some(cached) = files.get(&id) {
            return Ok(Arc::clone(cached));
        }
        files.put(id, Arc::clone(&file));
        Ok(file)
    }
}

/// Checksummed metadata for one immutable record. Preparing it performs no data
/// I/O; callers can account for the read before allocating its payload buffers.
pub struct ArchiveRecord<'a> {
    reader: &'a FreezerReader,
    entry: IndexEntry,
    raw_len: usize,
    read_bytes: usize,
}

impl<'a> ArchiveRecord<'a> {
    pub(crate) fn raw_len(&self) -> usize {
        self.raw_len
    }

    /// Compressed and decoded buffer sizes combined.
    pub fn read_bytes(&self) -> usize {
        self.read_bytes
    }

    /// Read and verify every chunk, rejecting an insufficient budget first.
    pub fn retrieve_limited(&self, max_bytes: usize) -> Result<Vec<u8>, Error> {
        self.read(max_bytes).map_err(crate::internal_error)
    }

    fn read(&self, max_bytes: usize) -> io::Result<Vec<u8>> {
        if self.read_bytes > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "archive read exceeds byte budget",
            ));
        }
        let file = self.reader.file(self.entry.file_id)?;
        let data = self.read_data(&file, 0..self.entry.stored_len as usize)?;
        // Validate the table before allocating the decoded record, including
        // on cache hits. Corrupt lengths must not trigger a large allocation.
        let table = if payload::table_len(self.raw_len)? == 0 {
            None
        } else {
            Some(ChunkTable::parse(
                &data,
                self.raw_len,
                data.len(),
                &self.entry.data_hash,
            )?)
        };
        let mut decoded = buffer(self.raw_len)?;
        self.reader
            .decoded
            .decode_into(&data, &self.entry.data_hash, &mut decoded, |decoded| {
                if let Some(table) = table {
                    for ((range, hash), output) in
                        table.chunks().zip(decoded.chunks_mut(CHUNK_SIZE))
                    {
                        payload::decode_into(&data[range], hash, output)?;
                    }
                    Ok(())
                } else {
                    payload::decode_into(&data, &self.entry.data_hash, decoded)
                }
            })?;
        Ok(decoded)
    }

    fn read_data(&self, file: &File, range: Range<usize>) -> io::Result<Vec<u8>> {
        fail::fail_point!("freezer-before-read-data");
        let mut data = buffer(range.len())?;
        read_exact_at(file, &mut data, self.entry.offset + range.start as u64)?;
        count_read(data.len() as u64);
        Ok(data)
    }

    pub(crate) fn ranges(self) -> io::Result<RecordRanges<'a>> {
        let table_len = payload::table_len(self.raw_len)?;
        let data = if table_len == 0 {
            RangeData::Whole(self.read(usize::MAX)?)
        } else {
            let file = self.reader.file(self.entry.file_id)?;
            let prefix_len = table_len
                .checked_add(lz4_flex::block::get_maximum_output_size(CHUNK_SIZE))
                .ok_or_else(|| invalid_data("archive prefix size overflow"))?
                .min(self.entry.stored_len as usize);
            // Fetch the table and first compressed chunk in one positional read.
            let mut prefix = self.read_data(&file, 0..prefix_len)?;
            let table = ChunkTable::parse(
                &prefix,
                self.raw_len,
                self.entry.stored_len as usize,
                &self.entry.data_hash,
            )?;
            let (range, hash) = table
                .chunks()
                .next()
                .ok_or_else(|| invalid_data("empty archive chunk table"))?;
            let mut first = buffer(CHUNK_SIZE)?;
            self.decode_chunk(&prefix[range], hash, &mut first)?;
            prefix.truncate(table_len);
            RangeData::Chunked {
                file,
                table: prefix,
                first,
            }
        };
        Ok(RecordRanges { record: self, data })
    }

    fn decode_chunk(&self, data: &[u8], hash: &[u8; 32], output: &mut [u8]) -> io::Result<()> {
        self.reader
            .decoded
            .decode_into(data, hash, output, |output| {
                payload::decode_into(data, hash, output)
            })
    }
}

enum RangeData {
    Whole(Vec<u8>),
    Chunked {
        file: Arc<File>,
        table: Vec<u8>,
        first: Vec<u8>,
    },
}

/// The first decoded chunk contains the usual block/transaction offset tables.
/// Borrowing it avoids both another I/O operation and temporary layout copies.
pub(crate) struct RecordRanges<'a> {
    record: ArchiveRecord<'a>,
    data: RangeData,
}

impl RecordRanges<'_> {
    pub fn len(&self) -> usize {
        self.record.raw_len
    }

    pub fn read(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
        if range.start > range.end || range.end > self.len() {
            return Err(invalid_data("archive byte range is outside the record"));
        }
        let (file, table, cached) = match &self.data {
            RangeData::Whole(raw) => return Ok(Cow::Borrowed(&raw[range])),
            RangeData::Chunked { file, table, first } => {
                if range.end <= first.len() {
                    return Ok(Cow::Borrowed(&first[range]));
                }
                (file, ChunkTable::validated(table), first)
            }
        };
        if range.is_empty() {
            return Ok(Cow::Borrowed(&[]));
        }
        let first = range.start.max(CHUNK_SIZE) / CHUNK_SIZE;
        let end = range.end.div_ceil(CHUNK_SIZE);
        let mut chunks = table.chunks().skip(first).take(end - first).peekable();
        let start_offset = chunks
            .peek()
            .ok_or_else(|| invalid_data("missing archive chunk"))?
            .0
            .start;
        let stop_offset = table
            .chunks()
            .nth(end - 1)
            .ok_or_else(|| invalid_data("missing archive chunk"))?
            .0
            .end;
        let data = self.record.read_data(file, start_offset..stop_offset)?;
        let mut output = buffer(range.len())?;
        if range.start < CHUNK_SIZE {
            output[..CHUNK_SIZE - range.start].copy_from_slice(&cached[range.start..]);
        }
        let mut scratch = Vec::new();
        for (index, (encoded, hash)) in (first..end).zip(chunks) {
            let chunk_start = index * CHUNK_SIZE;
            let chunk_end = (chunk_start + CHUNK_SIZE).min(self.len());
            let start = range.start.max(chunk_start);
            let end = range.end.min(chunk_end);
            let destination = &mut output[start - range.start..end - range.start];
            let compressed = &data[encoded.start - start_offset..encoded.end - start_offset];
            if start == chunk_start && end == chunk_end {
                self.record.decode_chunk(compressed, hash, destination)?;
            } else {
                if scratch.is_empty() {
                    scratch = buffer(CHUNK_SIZE)?;
                }
                let scratch = &mut scratch[..chunk_end - chunk_start];
                self.record.decode_chunk(compressed, hash, scratch)?;
                destination.copy_from_slice(&scratch[start - chunk_start..end - chunk_start]);
            }
        }
        Ok(Cow::Owned(output))
    }
}

fn count_read(bytes: u64) {
    if let Some(metrics) = ckb_metrics::handle() {
        metrics.ckb_freezer_read.inc_by(bytes);
    }
}
