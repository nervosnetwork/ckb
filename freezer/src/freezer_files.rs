use crate::format::{Commit, INDEX_ENTRY_SIZE, IndexEntry, checksum};
use crate::payload;
use crate::reader::FreezerReader;
use crate::storage::{data_path, invalid_data, sync_directory, write_commit};
use fail::fail_point;
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;

const MAX_FILE_SIZE: u64 = 2_000_000_000;
const OPEN_FILES_LIMIT: usize = 64;

/// The single writer for an append-only archive. Only `sync_all` commits data.
///
/// A failed write or sync requires reopening the archive. Readers can continue
/// to read the previously committed prefix. Dropping the writer does not commit.
pub struct FreezerFiles {
    pub(crate) reader: Arc<FreezerReader>,
    head: File,
    index: File,
    pending: Commit,
    pending_size: u64,
    max_file_size: u64,
    failed: bool,
}

impl FreezerFiles {
    /// Opens an archive with the default file and cache limits.
    pub fn open(path: PathBuf) -> io::Result<Self> {
        FreezerFilesBuilder::new(path).build()
    }

    /// The next item number, including this writer's uncommitted appends.
    pub fn number(&self) -> u64 {
        self.pending.count + 1
    }

    /// The next item number after the durable, publicly readable prefix.
    pub fn committed_number(&self) -> u64 {
        self.reader.number()
    }

    /// Appends a checksummed LZ4 record. Call `sync_all` before relying on it.
    pub fn append(&mut self, number: u64, input: &[u8]) -> io::Result<()> {
        self.check_writable()?;
        if number != self.number() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("expected archive item {}, got {number}", self.number()),
            ));
        }
        let (data, data_hash) = payload::encode(input)?;
        let stored_len = u32::try_from(data.len()).map_err(invalid_data)?;
        let count = self
            .pending
            .count
            .checked_add(1)
            .ok_or_else(|| invalid_data("archive item number overflow"))?;
        count
            .checked_add(1)
            .ok_or_else(|| invalid_data("archive is full"))?;
        count
            .checked_mul(INDEX_ENTRY_SIZE)
            .ok_or_else(|| invalid_data("archive index is full"))?;
        let size = self
            .pending_size
            .checked_add(u64::from(stored_len) + INDEX_ENTRY_SIZE)
            .ok_or_else(|| invalid_data("archive size overflow"))?;
        self.failed = true;
        self.append_inner(number, input.len() as u64, stored_len, data_hash, &data)?;
        self.pending_size = size;
        self.failed = false;
        Ok(())
    }

    fn append_inner(
        &mut self,
        number: u64,
        raw_len: u64,
        stored_len: u32,
        data_hash: [u8; 32],
        data: &[u8],
    ) -> io::Result<()> {
        let end = self
            .pending
            .offset
            .checked_add(u64::from(stored_len))
            .ok_or_else(|| invalid_data("archive file offset overflow"))?;
        if self.pending.offset != 0 && end > self.max_file_size {
            // Once a head is sealed we never write it again. Sync it before
            // releasing its only writer; syncing the newest head is insufficient.
            fail_point!("freezer-before-seal-sync", |_| Err(io::Error::other(
                "injected seal sync failure"
            )));
            self.head.sync_all()?;
            fail_point!("freezer-after-seal-sync");
            let id = self
                .pending
                .file_id
                .checked_add(1)
                .ok_or_else(|| invalid_data("archive file id overflow"))?;
            fail_point!("freezer-before-create-file", |_| Err(io::Error::other(
                "injected create failure"
            )));
            let head = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(data_path(&self.reader.path, id))?;
            fail_point!("freezer-after-create-file");
            self.head = head;
            self.pending.file_id = id;
            self.pending.offset = 0;
        }
        let entry = IndexEntry {
            number,
            file_id: self.pending.file_id,
            offset: self.pending.offset,
            stored_len,
            raw_len,
            data_hash,
        };
        fail_point!("freezer-before-data-write", |_| Err(io::Error::other(
            "injected data write failure"
        )));
        fail_point!("freezer-partial-data-write", |_| {
            self.head.write_all(&data[..data.len() / 2])?;
            Err(io::Error::other("injected partial data write"))
        });
        self.head.write_all(data)?;
        fail_point!("freezer-after-data-write");
        let encoded = entry.encode();
        fail_point!("freezer-before-index-write", |_| Err(io::Error::other(
            "injected index write failure"
        )));
        fail_point!("freezer-partial-index-write", |_| {
            self.index.write_all(&encoded[..encoded.len() / 2])?;
            Err(io::Error::other("injected partial index write"))
        });
        self.index.write_all(&encoded)?;
        fail_point!("freezer-after-index-write");
        self.pending.count = number;
        self.pending.offset += u64::from(stored_len);
        self.pending.index_hash = checksum(&encoded);
        Ok(())
    }

    /// Commits all appends: data, index and file names precede the commit record.
    ///
    /// An error makes the outcome uncertain until reopen. No additional write
    /// is accepted, and the reader's published prefix is left unchanged.
    pub fn sync_all(&mut self) -> io::Result<()> {
        self.check_writable()?;
        if self.number() == self.committed_number() {
            return Ok(());
        }
        self.failed = true;
        fail_point!("freezer-before-head-sync", |_| Err(io::Error::other(
            "injected head sync failure"
        )));
        self.head.sync_all()?;
        fail_point!("freezer-after-head-sync");
        fail_point!("freezer-before-index-sync", |_| Err(io::Error::other(
            "injected index sync failure"
        )));
        self.index.sync_all()?;
        fail_point!("freezer-after-index-sync");
        sync_directory(&self.reader.path)?;
        fail_point!("freezer-after-data-directory-sync");
        write_commit(&self.reader.path, &self.pending)?;
        self.reader.publish(self.pending.count);
        self.update_size_metric();
        self.failed = false;
        Ok(())
    }

    fn update_size_metric(&self) {
        if let Some(metrics) = ckb_metrics::handle() {
            metrics
                .ckb_freezer_size
                .set(i64::try_from(self.pending_size).unwrap_or(i64::MAX));
        }
    }

    /// Retrieves a committed item. Pending appends are not visible.
    pub fn retrieve(&self, number: u64) -> io::Result<Option<Vec<u8>>> {
        self.reader.retrieve(number)
    }

    /// Checks every committed record, including its payload and decompression.
    pub fn verify(&self) -> io::Result<()> {
        for number in 1..self.committed_number() {
            self.retrieve(number)?
                .ok_or_else(|| invalid_data("committed archive item missing"))?;
        }
        Ok(())
    }

    fn check_writable(&self) -> io::Result<()> {
        if self.failed {
            Err(io::Error::other(
                "archive write failed; reopen before writing again",
            ))
        } else {
            Ok(())
        }
    }
}

/// Configuration for an archive writer and its bounded read cache.
pub struct FreezerFilesBuilder {
    path: PathBuf,
    max_file_size: u64,
    open_files_limit: usize,
}

impl FreezerFilesBuilder {
    /// Uses independent LZ4 blocks and the default file and cache limits.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            max_file_size: MAX_FILE_SIZE,
            open_files_limit: OPEN_FILES_LIMIT,
        }
    }

    /// Sets the segment size. A single larger item occupies its own segment.
    pub fn max_file_size(mut self, size: u64) -> Self {
        self.max_file_size = size;
        self
    }

    /// Limits cached data file handles. Active reads may temporarily retain more.
    pub fn open_files_limit(mut self, limit: usize) -> Self {
        self.open_files_limit = limit;
        self
    }

    /// Opens the committed archive and discards only its uncommitted tail.
    pub fn build(self) -> io::Result<FreezerFiles> {
        if self.max_file_size == 0 || self.open_files_limit == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "archive limits must be positive",
            ));
        }
        crate::storage::create_directory(&self.path)?;
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.path.join("FLOCK"))?;
        FileExt::try_lock_exclusive(&lock)?;
        let commit = crate::storage::open_commit(&self.path)?;
        let mut index = OpenOptions::new()
            .read(true)
            .write(true)
            .create(commit.count == 0)
            .truncate(false)
            .open(self.path.join("INDEX"))?;
        let pending_size = validate_index(&self.path, &index, &commit)?;

        // No committed data is repaired or guessed. Only bytes after the commit
        // boundary can be removed. This cleanup is safe to repeat after a crash.
        let mut head = OpenOptions::new()
            .read(true)
            .write(true)
            .create(commit.count == 0)
            .truncate(false)
            .open(data_path(&self.path, commit.file_id))?;
        index.set_len(commit.count * INDEX_ENTRY_SIZE)?;
        fail_point!("freezer-after-recovery-index-truncate");
        head.set_len(commit.offset)?;
        fail_point!("freezer-after-recovery-head-truncate");
        index.seek(SeekFrom::End(0))?;
        head.seek(SeekFrom::End(0))?;
        crate::storage::remove_uncommitted_files(&self.path, commit.file_id)?;
        fail_point!("freezer-after-recovery-remove-files");
        head.sync_all()?;
        index.sync_all()?;
        sync_directory(&self.path)?;
        fail_point!("freezer-after-recovery-sync");
        // `try_clone` shares the writer's cursor. Windows positional reads can
        // change that cursor, so the reader needs a separate open file object.
        let read_index = File::open(self.path.join("INDEX"))?;
        let reader = Arc::new(FreezerReader::new(
            self.path,
            read_index,
            commit.count,
            self.open_files_limit,
            lock,
        ));
        let files = FreezerFiles {
            reader,
            head,
            index,
            pending: commit,
            pending_size,
            max_file_size: self.max_file_size,
            failed: false,
        };
        files.update_size_metric();
        Ok(files)
    }
}

fn validate_index(path: &std::path::Path, index: &File, commit: &Commit) -> io::Result<u64> {
    let index_len = commit
        .count
        .checked_mul(INDEX_ENTRY_SIZE)
        .ok_or_else(|| invalid_data("invalid committed index length"))?;
    if index.metadata()?.len() < index_len {
        return Err(invalid_data("committed archive index is truncated"));
    }
    let mut previous = Commit::default();
    let mut size = index_len;
    let mut raw = [0; INDEX_ENTRY_SIZE as usize];
    let mut reader = BufReader::with_capacity(64 * 1024, index);
    reader.seek(SeekFrom::Start(0))?;
    for number in 1..=commit.count {
        reader.read_exact(&mut raw)?;
        let entry = IndexEntry::decode(&raw)?;
        if entry.number != number || entry.stored_len == 0 {
            return Err(invalid_data(format!("invalid archive index item {number}")));
        }
        size = size
            .checked_add(u64::from(entry.stored_len))
            .ok_or_else(|| invalid_data("archive size overflow"))?;
        if entry.file_id == previous.file_id {
            if entry.offset != previous.offset {
                return Err(invalid_data("non-contiguous archive index"));
            }
        } else {
            if previous.count == 0
                || previous.file_id.checked_add(1) != Some(entry.file_id)
                || entry.offset != 0
            {
                return Err(invalid_data("invalid archive segment transition"));
            }
            if fs::metadata(data_path(path, previous.file_id))?.len() != previous.offset {
                return Err(invalid_data(
                    "committed archive segment has the wrong length",
                ));
            }
        }
        previous = Commit {
            count: number,
            file_id: entry.file_id,
            offset: entry
                .offset
                .checked_add(u64::from(entry.stored_len))
                .ok_or_else(|| invalid_data("invalid archive item bounds"))?,
            index_hash: [0; 32],
        };
    }
    if commit.count != 0 {
        previous.index_hash = checksum(&raw);
    }
    if &previous != commit {
        return Err(invalid_data("archive commit does not match its index"));
    }
    if commit.count != 0 && fs::metadata(data_path(path, commit.file_id))?.len() < commit.offset {
        return Err(invalid_data("committed archive data is truncated"));
    }
    Ok(size)
}
