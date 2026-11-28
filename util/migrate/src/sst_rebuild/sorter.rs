use ckb_db_schema::{
    BLOCK_BODY_KEY_SIZE, BLOCK_HASH_SIZE, BLOCK_KEY_SIZE, BLOCK_NUMBER_KEY_SIZE, Col,
};
use ckb_error::Error;
use ckb_logger::info;
use ckb_types::core::BlockNumber;
use rayon::prelude::*;
use snap::{read::FrameDecoder, write::FrameEncoder};
use std::collections::{BTreeMap, BinaryHeap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{
    BlockHashBytes, BlockKey, GeneratedSst, SplitSstWriter, SstIngestor, SstWriterOptions,
    block_key_bytes, internal_error, io_err,
};

const SHARD_MEM_SORT_BYTES: usize = 128 * 1024 * 1024;
const MAX_MERGE_FAN_IN: usize = 16;
const SPILL_IO_BUF_BYTES: usize = 256 * 1024;
const MERGE_RUN_BUF_BYTES: usize = 64 * 1024;
const BLOCK_BODY_BATCH_HEADER_BYTES: usize = 4;
const BLOCK_BODY_TX_HEADER_BYTES: usize = 8;

/// Reusable encoder for one block's transactions as a single spill value:
/// `tx_count u32 BE || (tx_index u32 BE || value_len u32 BE || value)*`.
///
/// The matching spill key is `block_key_bytes(number, hash)`, so sorting and
/// merging spill records orders whole blocks without touching individual txs.
pub(super) struct PendingBlockBody {
    pub(super) hash: BlockHashBytes,
    number: Option<BlockNumber>,
    buf: Vec<u8>,
    tx_count: u32,
    pub(super) active: bool,
}

impl PendingBlockBody {
    pub(super) fn new() -> Self {
        Self {
            hash: [0u8; BLOCK_HASH_SIZE],
            number: None,
            buf: Vec::new(),
            tx_count: 0,
            active: false,
        }
    }

    pub(super) fn reset(&mut self, hash: BlockHashBytes, number: Option<BlockNumber>) {
        self.hash = hash;
        self.number = number;
        self.tx_count = 0;
        self.active = true;
        self.buf.clear();
        if number.is_some() {
            self.buf.resize(BLOCK_BODY_BATCH_HEADER_BYTES, 0);
        }
    }

    pub(super) fn push_tx(&mut self, index: u32, value: &[u8]) -> Result<(), Error> {
        self.tx_count = self
            .tx_count
            .checked_add(1)
            .ok_or_else(|| internal_error("block body transaction count overflows u32"))?;
        if self.number.is_some() {
            let value_len = u32::try_from(value.len()).map_err(|_| {
                internal_error(format!(
                    "transaction value is too large: {} bytes",
                    value.len()
                ))
            })?;
            self.buf.reserve(BLOCK_BODY_TX_HEADER_BYTES + value.len());
            self.buf.extend_from_slice(&index.to_be_bytes());
            self.buf.extend_from_slice(&value_len.to_be_bytes());
            self.buf.extend_from_slice(value);
        }
        Ok(())
    }

    fn encode_batch(&mut self) -> Result<Box<[u8]>, Error> {
        if self.number.is_none() {
            return Err(internal_error(
                "orphan block body spill record cannot be encoded",
            ));
        }
        if self.tx_count == 0 {
            return Err(internal_error(
                "block body spill record has no transactions",
            ));
        }
        self.buf[..BLOCK_BODY_BATCH_HEADER_BYTES].copy_from_slice(&self.tx_count.to_be_bytes());
        Ok(Box::from(self.buf.as_slice()))
    }

    pub(super) fn flush_into(
        &mut self,
        sorter: &mut ShardSorter<'_>,
        migrated: &mut u64,
        skipped_orphans: &mut u64,
    ) -> Result<(), Error> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        let tx_count = u64::from(self.tx_count);
        match self.number {
            Some(number) => {
                let key = block_key_bytes(number, &self.hash);
                let value = self.encode_batch()?;
                sorter.push(number, key, value)?;
                *migrated += tx_count;
            }
            None => *skipped_orphans += tx_count,
        }
        Ok(())
    }
}

/// Expands one block-level spill record into per-transaction V1 block-body keys.
pub(super) fn expand_block_body_batch(
    block_key: &BlockKey,
    batch: &[u8],
    key_buf: &mut [u8; BLOCK_BODY_KEY_SIZE],
    mut put: impl FnMut(&[u8], &[u8]) -> Result<(), Error>,
) -> Result<(), Error> {
    let Some(header) = batch.get(..BLOCK_BODY_BATCH_HEADER_BYTES) else {
        return Err(internal_error(format!(
            "block body spill value is too short: {} bytes",
            batch.len()
        )));
    };
    let tx_count = u32::from_be_bytes(header.try_into().expect("fixed-length batch header"));
    if tx_count == 0 {
        return Err(internal_error(
            "block body spill value contains no transactions",
        ));
    }

    key_buf[..BLOCK_KEY_SIZE].copy_from_slice(block_key);
    let mut offset = BLOCK_BODY_BATCH_HEADER_BYTES;
    let mut last_index: Option<u32> = None;
    for _ in 0..tx_count {
        let header_end = offset
            .checked_add(BLOCK_BODY_TX_HEADER_BYTES)
            .filter(|end| *end <= batch.len())
            .ok_or_else(|| {
                internal_error(format!(
                    "truncated block body spill value at offset {offset}"
                ))
            })?;
        let index = u32::from_be_bytes(
            batch[offset..offset + 4]
                .try_into()
                .expect("fixed-length transaction index"),
        );
        let value_len = u32::from_be_bytes(
            batch[offset + 4..header_end]
                .try_into()
                .expect("fixed-length transaction value length"),
        ) as usize;
        if let Some(last) = last_index
            && index <= last
        {
            return Err(internal_error(format!(
                "block body spill value has non-increasing transaction indexes: {index} after {last}"
            )));
        }
        last_index = Some(index);
        offset = header_end;

        let value_end = offset
            .checked_add(value_len)
            .filter(|end| *end <= batch.len())
            .ok_or_else(|| {
                internal_error(format!(
                    "truncated block body spill value at offset {offset}"
                ))
            })?;
        key_buf[BLOCK_KEY_SIZE..BLOCK_BODY_KEY_SIZE].copy_from_slice(&index.to_be_bytes());
        put(key_buf, &batch[offset..value_end])?;
        offset = value_end;
    }
    if offset != batch.len() {
        return Err(internal_error(format!(
            "block body spill value has {} trailing bytes",
            batch.len() - offset
        )));
    }
    Ok(())
}

pub(super) fn make_block_body_emitter(
    start: BlockNumber,
    end: BlockNumber,
) -> impl FnMut(&mut SplitSstWriter<'_>, BlockKey, &[u8]) -> Result<(), Error> {
    let mut key_buf = [0u8; BLOCK_BODY_KEY_SIZE];
    let mut last_block_key: Option<BlockKey> = None;
    move |writer, key, value| {
        let number = u64::from_be_bytes(
            key[..BLOCK_NUMBER_KEY_SIZE]
                .try_into()
                .expect("fixed-length block number slice"),
        );
        if number < start || number >= end {
            return Err(internal_error(format!(
                "block body spill record number {number} is outside shard range {start}..{end}"
            )));
        }
        if let Some(last) = last_block_key
            && key <= last
        {
            return Err(internal_error(
                "block body spill records are not strictly increasing",
            ));
        }
        last_block_key = Some(key);
        expand_block_body_batch(&key, value, &mut key_buf, |tx_key, tx_val| {
            writer.put(tx_key, tx_val)
        })
    }
}

// ---------------------------------------------------------------------------
// Partitioned external sorter (40-byte BlockKey, Snappy-framed spills, bounded RAM & FDs)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Entry {
    key: BlockKey,
    value: Box<[u8]>,
}

impl Entry {
    #[inline]
    fn record_bytes(&self) -> usize {
        BLOCK_KEY_SIZE + 4 + self.value.len()
    }
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for Entry {}

impl Ord for Entry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key.cmp(&other.key)
    }
}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

struct MergeItem {
    entry: Entry,
    run: usize,
}

impl PartialEq for MergeItem {
    fn eq(&self, other: &Self) -> bool {
        self.entry == other.entry && self.run == other.run
    }
}

impl Eq for MergeItem {}

impl Ord for MergeItem {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .entry
            .cmp(&self.entry)
            .then_with(|| other.run.cmp(&self.run))
    }
}

impl PartialOrd for MergeItem {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy)]
pub(super) struct ShardSortLimits {
    pub(super) range_blocks: u64,
    pub(super) flush_bytes: usize,
    pub(super) shard_mem_bytes: usize,
    pub(super) max_merge_fan_in: usize,
    pub(super) disk_sort_concurrency: usize,
}

impl ShardSortLimits {
    pub(super) const fn for_column(
        range_blocks: u64,
        flush_bytes: usize,
        disk_sort_concurrency: usize,
    ) -> Self {
        Self {
            range_blocks,
            flush_bytes,
            shard_mem_bytes: SHARD_MEM_SORT_BYTES,
            max_merge_fan_in: MAX_MERGE_FAN_IN,
            disk_sort_concurrency,
        }
    }
}

struct ShardData {
    start: BlockNumber,
    mem_entries: Vec<Entry>,
    mem_bytes: usize,
    part_path: Option<PathBuf>,
    part_bytes: u64,
}

pub(super) struct ShardSorter<'a> {
    pool: &'a rayon::ThreadPool,
    col: Col,
    dir: PathBuf,
    limits: ShardSortLimits,
    buffers: BTreeMap<BlockNumber, (Vec<Entry>, usize)>,
    part_files: BTreeMap<BlockNumber, (PathBuf, u64)>,
    buffered_bytes: usize,
}

impl<'a> ShardSorter<'a> {
    pub(super) fn new(
        pool: &'a rayon::ThreadPool,
        sst_root: &Path,
        col: Col,
        limits: ShardSortLimits,
    ) -> Self {
        Self {
            pool,
            col,
            dir: sst_root.join("spill").join(col),
            limits,
            buffers: BTreeMap::new(),
            part_files: BTreeMap::new(),
            buffered_bytes: 0,
        }
    }

    pub(super) fn push(
        &mut self,
        number: BlockNumber,
        key: BlockKey,
        value: Box<[u8]>,
    ) -> Result<(), Error> {
        let start = number / self.limits.range_blocks * self.limits.range_blocks;
        let entry = Entry { key, value };
        let entry_bytes = entry.record_bytes();
        let (entries, bytes) = self.buffers.entry(start).or_default();
        entries.push(entry);
        *bytes += entry_bytes;
        self.buffered_bytes += entry_bytes;

        if self.buffered_bytes >= self.limits.flush_bytes {
            self.flush_to_partitions()?;
        }
        Ok(())
    }

    /// Appends buffered entries into one Snappy-framed partition file per shard without sorting
    /// during scan, reusing shard vector capacities across flushes. Compresses and writes shards
    /// in parallel up to `limits.disk_sort_concurrency` to keep FDs bounded while saturating I/O.
    fn flush_to_partitions(&mut self) -> Result<(), Error> {
        if self.buffered_bytes == 0 {
            return Ok(());
        }
        fs::create_dir_all(&self.dir).map_err(|e| io_err("create spill dir", &self.dir, e))?;

        let dir = &self.dir;
        let flush_one = |start: BlockNumber,
                         entries: &mut Vec<Entry>,
                         bytes: &mut usize|
         -> Result<(BlockNumber, PathBuf, u64), Error> {
            let path = dir.join(format!("{start:016x}.part"));
            let mut writer = open_spill_writer(&path, true)?;
            for entry in entries.drain(..) {
                write_entry_io(&mut writer, &entry.key, &entry.value)
                    .map_err(|e| io_err("write partition file", &path, e))?;
            }
            writer
                .flush()
                .map_err(|e| io_err("flush partition file", &path, e))?;
            let flushed = *bytes as u64;
            *bytes = 0;
            Ok((start, path, flushed))
        };

        let mut pending: Vec<(BlockNumber, &mut Vec<Entry>, &mut usize)> = self
            .buffers
            .iter_mut()
            .filter_map(|(&start, (entries, bytes))| {
                (!entries.is_empty()).then_some((start, entries, bytes))
            })
            .collect();

        let concurrency = self.limits.disk_sort_concurrency.max(1);
        if concurrency == 1 || pending.len() <= 1 {
            for (start, entries, bytes) in pending {
                let (start, path, flushed) = flush_one(start, entries, bytes)?;
                let stat = self.part_files.entry(start).or_insert((path, 0));
                stat.1 += flushed;
            }
        } else {
            for batch in pending.chunks_mut(concurrency) {
                let results: Vec<Result<(BlockNumber, PathBuf, u64), Error>> =
                    self.pool.install(|| {
                        batch
                            .par_iter_mut()
                            .map(|(start, entries, bytes)| flush_one(*start, entries, bytes))
                            .collect()
                    });
                for result in results {
                    let (start, path, flushed) = result?;
                    let stat = self.part_files.entry(start).or_insert((path, 0));
                    stat.1 += flushed;
                }
            }
        }

        self.buffered_bytes = 0;
        Ok(())
    }

    pub(super) fn finish(
        self,
        sst_root: &Path,
        ingestor: &SstIngestor<'_>,
        sst_options: &SstWriterOptions,
    ) -> Result<(), Error> {
        self.finish_with_emitter(sst_root, ingestor, sst_options, |_start, _end| {
            |writer: &mut SplitSstWriter<'_>, key: BlockKey, value: &[u8]| writer.put(&key, value)
        })
    }

    pub(super) fn finish_with_emitter<M, E>(
        mut self,
        sst_root: &Path,
        ingestor: &SstIngestor<'_>,
        sst_options: &SstWriterOptions,
        make_emitter: M,
    ) -> Result<(), Error>
    where
        M: Fn(BlockNumber, BlockNumber) -> E + Sync,
        E: FnMut(&mut SplitSstWriter<'_>, BlockKey, &[u8]) -> Result<(), Error>,
    {
        let mut keys: Vec<BlockNumber> = self
            .buffers
            .iter()
            .filter_map(|(&start, (entries, _))| (!entries.is_empty()).then_some(start))
            .chain(self.part_files.keys().copied())
            .collect();
        keys.sort_unstable();
        keys.dedup();
        if keys.is_empty() {
            return Ok(());
        }

        let (mut mem_only, mut spilled) = (Vec::new(), Vec::new());
        for start in keys {
            let (mem_entries, mem_bytes) = self.buffers.remove(&start).unwrap_or_default();
            let (part_path, part_bytes) = match self.part_files.remove(&start) {
                Some((path, bytes)) => (Some(path), bytes),
                None => (None, 0),
            };
            let shard = ShardData {
                start,
                mem_entries,
                mem_bytes,
                part_path,
                part_bytes,
            };
            if shard.part_path.is_some() {
                spilled.push(shard);
            } else {
                mem_only.push(shard);
            }
        }

        let total = mem_only.len() + spilled.len();
        let completed = AtomicUsize::new(0);
        let progress_interval = (total / 20).max(1);
        info!(
            "SST rebuild: encoding {total} shards for column {} ({} in-memory, {} partitioned)",
            self.col,
            mem_only.len(),
            spilled.len()
        );

        let process_shard = |shard: ShardData| -> Result<(), Error> {
            let end = shard.start.saturating_add(self.limits.range_blocks);
            let emit = make_emitter(shard.start, end);
            let ssts = encode_shard(
                self.col,
                &self.dir,
                sst_root,
                shard,
                end,
                self.limits,
                sst_options,
                emit,
            )?;
            ingestor.ingest_batch(self.col, ssts)?;
            let done = completed.fetch_add(1, Ordering::SeqCst) + 1;
            if done == total || done.is_multiple_of(progress_interval) {
                info!(
                    "SST rebuild: encoded and ingested shards {done}/{total} for column {}",
                    self.col
                );
            }
            Ok(())
        };

        run_shards_in_pool(self.pool, mem_only, &process_shard)?;
        let disk_concurrency = self.limits.disk_sort_concurrency.max(1);
        if disk_concurrency == 1 {
            for shard in spilled {
                process_shard(shard)?;
            }
        } else {
            let mut spilled_iter = spilled.into_iter();
            loop {
                let batch: Vec<_> = spilled_iter.by_ref().take(disk_concurrency).collect();
                if batch.is_empty() {
                    break;
                }
                run_shards_in_pool(self.pool, batch, &process_shard)?;
            }
        }

        let _ = fs::remove_dir_all(&self.dir);
        Ok(())
    }
}

fn run_shards_in_pool<F>(
    pool: &rayon::ThreadPool,
    shards: Vec<ShardData>,
    process: &F,
) -> Result<(), Error>
where
    F: Fn(ShardData) -> Result<(), Error> + Sync,
{
    if shards.is_empty() {
        return Ok(());
    }
    let results: Vec<Result<(), Error>> =
        pool.install(|| shards.into_par_iter().map(process).collect());
    for result in results {
        result?;
    }
    Ok(())
}

fn open_spill_writer(path: &Path, append: bool) -> Result<FrameEncoder<BufWriter<File>>, Error> {
    let mut opts = OpenOptions::new();
    opts.create(true);
    if append {
        opts.append(true);
    } else {
        opts.write(true).truncate(true);
    }
    let file = opts
        .open(path)
        .map_err(|e| io_err("open spill file", path, e))?;
    Ok(FrameEncoder::new(BufWriter::with_capacity(
        SPILL_IO_BUF_BYTES,
        file,
    )))
}

fn open_spill_reader(
    path: &Path,
    buf_bytes: usize,
) -> Result<FrameDecoder<BufReader<File>>, Error> {
    let file = File::open(path).map_err(|e| io_err("open spill file", path, e))?;
    Ok(FrameDecoder::new(BufReader::with_capacity(buf_bytes, file)))
}

#[allow(clippy::too_many_arguments)]
fn encode_shard<E>(
    col: Col,
    spill_dir: &Path,
    sst_root: &Path,
    mut shard: ShardData,
    end: BlockNumber,
    limits: ShardSortLimits,
    sst_options: &SstWriterOptions,
    mut emit: E,
) -> Result<Vec<GeneratedSst>, Error>
where
    E: FnMut(&mut SplitSstWriter<'_>, BlockKey, &[u8]) -> Result<(), Error>,
{
    let mut writer = SplitSstWriter::new(
        sst_options,
        col,
        sst_root,
        format!("{:016x}-{end:016x}", shard.start),
    )?;

    let total_bytes = shard.part_bytes.saturating_add(shard.mem_bytes as u64);

    // Fast path: shard fits in memory (no spill file, or single partition file <= shard_mem_bytes).
    if shard.part_path.is_none() || total_bytes <= limits.shard_mem_bytes as u64 {
        if let Some(part_path) = shard.part_path.take() {
            read_all_entries_into(&part_path, &mut shard.mem_entries)?;
            let _ = fs::remove_file(&part_path);
        }
        shard.mem_entries.sort_unstable();
        for entry in shard.mem_entries {
            emit(&mut writer, entry.key, &entry.value)?;
        }
        return writer.finish();
    }

    // Large shard path: split single partition file into bounded sorted runs, then k-way merge
    // with a hard fan-in cap so open file descriptors remain strictly bounded.
    let part_path = shard.part_path.take().expect("part_path checked");
    let mut runs = create_sorted_runs(
        spill_dir,
        shard.start,
        &part_path,
        shard.mem_entries,
        limits.shard_mem_bytes,
    )?;
    let _ = fs::remove_file(&part_path);

    let mut merge_pass = 0u64;
    while runs.len() > limits.max_merge_fan_in {
        let mut next_runs = Vec::with_capacity(runs.len().div_ceil(limits.max_merge_fan_in));
        for (chunk_idx, chunk) in runs.chunks(limits.max_merge_fan_in).enumerate() {
            let out_path = spill_dir.join(format!(
                "{:016x}-p{merge_pass:03}-{chunk_idx:06}.run",
                shard.start
            ));
            merge_runs_to_file(chunk, &out_path)?;
            for path in chunk {
                let _ = fs::remove_file(path);
            }
            next_runs.push(out_path);
        }
        runs = next_runs;
        merge_pass += 1;
    }

    merge_runs_into(&runs, |entry| emit(&mut writer, entry.key, &entry.value))?;
    for path in &runs {
        let _ = fs::remove_file(path);
    }
    writer.finish()
}

fn read_all_entries_into(path: &Path, out: &mut Vec<Entry>) -> Result<(), Error> {
    let mut reader = open_spill_reader(path, SPILL_IO_BUF_BYTES)?;
    while let Some(entry) =
        read_entry_io(&mut reader).map_err(|e| io_err("read partition file", path, e))?
    {
        out.push(entry);
    }
    Ok(())
}

fn write_sorted_run(entries: &mut Vec<Entry>, run_path: &Path) -> Result<(), Error> {
    entries.sort_unstable();
    let mut writer = open_spill_writer(run_path, false)?;
    for entry in entries.drain(..) {
        write_entry_io(&mut writer, &entry.key, &entry.value)
            .map_err(|e| io_err("write sorted run", run_path, e))?;
    }
    writer
        .flush()
        .map_err(|e| io_err("flush sorted run", run_path, e))
}

fn create_sorted_runs(
    spill_dir: &Path,
    shard_start: BlockNumber,
    part_path: &Path,
    mem_entries: Vec<Entry>,
    run_bytes_limit: usize,
) -> Result<Vec<PathBuf>, Error> {
    let mut reader = open_spill_reader(part_path, SPILL_IO_BUF_BYTES)?;
    let mut runs = Vec::new();
    let mut chunk: Vec<Entry> = Vec::new();
    let mut chunk_bytes = 0usize;

    let mut push_entry = |entry: Entry| -> Result<(), Error> {
        chunk_bytes += entry.record_bytes();
        chunk.push(entry);
        if chunk_bytes >= run_bytes_limit {
            let run_path = spill_dir.join(format!("{shard_start:016x}-r{:06}.run", runs.len()));
            write_sorted_run(&mut chunk, &run_path)?;
            runs.push(run_path);
            chunk_bytes = 0;
        }
        Ok(())
    };

    while let Some(entry) =
        read_entry_io(&mut reader).map_err(|e| io_err("read partition file", part_path, e))?
    {
        push_entry(entry)?;
    }
    for entry in mem_entries {
        push_entry(entry)?;
    }
    if !chunk.is_empty() {
        let run_path = spill_dir.join(format!("{shard_start:016x}-r{:06}.run", runs.len()));
        write_sorted_run(&mut chunk, &run_path)?;
        runs.push(run_path);
    }
    Ok(runs)
}

fn merge_runs_to_file(runs: &[PathBuf], out_path: &Path) -> Result<(), Error> {
    let mut writer = open_spill_writer(out_path, false)?;
    merge_runs_into(runs, |entry| {
        write_entry_io(&mut writer, &entry.key, &entry.value)
            .map_err(|e| io_err("write merged run", out_path, e))
    })?;
    writer
        .flush()
        .map_err(|e| io_err("flush merged run", out_path, e))
}

fn merge_runs_into<F>(runs: &[PathBuf], mut emit: F) -> Result<(), Error>
where
    F: FnMut(Entry) -> Result<(), Error>,
{
    let mut readers = Vec::with_capacity(runs.len());
    let mut heap = BinaryHeap::with_capacity(runs.len());

    for (idx, path) in runs.iter().enumerate() {
        let mut reader = open_spill_reader(path, MERGE_RUN_BUF_BYTES)?;
        if let Some(entry) =
            read_entry_io(&mut reader).map_err(|e| io_err("read run file", path, e))?
        {
            heap.push(MergeItem { entry, run: idx });
        }
        readers.push((path, reader));
    }

    while let Some(MergeItem { entry, run }) = heap.pop() {
        emit(entry)?;
        let (path, reader) = &mut readers[run];
        if let Some(next) = read_entry_io(reader).map_err(|e| io_err("read run file", path, e))? {
            heap.push(MergeItem { entry: next, run });
        }
    }
    Ok(())
}

fn write_entry_io(writer: &mut impl Write, key: &BlockKey, value: &[u8]) -> io::Result<()> {
    let value_len = u32::try_from(value.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "spill value too large"))?;
    writer
        .write_all(key)
        .and_then(|_| writer.write_all(&value_len.to_le_bytes()))
        .and_then(|_| writer.write_all(value))
}

fn read_entry_io(reader: &mut impl Read) -> io::Result<Option<Entry>> {
    let mut key = [0u8; BLOCK_KEY_SIZE];
    loop {
        match reader.read(&mut key[..1]) {
            Ok(0) => return Ok(None),
            Ok(1) => break,
            Ok(_) => unreachable!(),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    reader.read_exact(&mut key[1..])?;

    let mut value_len_bytes = [0u8; 4];
    reader.read_exact(&mut value_len_bytes)?;
    let value_len = u32::from_le_bytes(value_len_bytes) as usize;
    let mut value = vec![0u8; value_len];
    reader.read_exact(&mut value)?;

    Ok(Some(Entry {
        key,
        value: value.into_boxed_slice(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sst_rebuild::BODY_RANGE_BLOCKS;
    use ckb_db::{DBIterator, IteratorMode, RocksDB, Schema};
    use ckb_db_schema::{COLUMN_BLOCK_BODY, COLUMN_BLOCK_EXT, block_body_key};
    use ckb_types::packed;

    fn expand_test_batch(batch: &[u8]) -> Result<(), Error> {
        let block_key = block_key_bytes(0, &[0u8; BLOCK_HASH_SIZE]);
        let mut key_buf = [0u8; BLOCK_BODY_KEY_SIZE];
        expand_block_body_batch(&block_key, batch, &mut key_buf, |_, _| Ok(()))
    }

    #[test]
    fn spill_entry_roundtrips_and_rejects_truncated_header() {
        let mut bytes = Vec::new();
        let key = [0x11u8; BLOCK_KEY_SIZE];
        write_entry_io(&mut bytes, &key, b"small-value").unwrap();

        let mut cursor = io::Cursor::new(bytes);
        let entry = read_entry_io(&mut cursor).unwrap().unwrap();
        assert_eq!(entry.key, key);
        assert_eq!(entry.value.as_ref(), b"small-value");
        assert!(read_entry_io(&mut cursor).unwrap().is_none());

        for partial_len in 1..BLOCK_KEY_SIZE + 4 {
            let mut partial = io::Cursor::new(vec![0xAB; partial_len]);
            let err = read_entry_io(&mut partial).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        }
    }

    #[test]
    fn shard_sorter_handles_in_memory_single_part_and_multi_pass_fan_in_merge() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("target");
        let sst_root = tmp.path().join("sst");
        let target = RocksDB::create_for_bulk_load_open(&db_path, Schema::V1).unwrap();
        let ingestor = SstIngestor::new(&target);
        let sst_options = SstWriterOptions::load();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .unwrap();

        // Tiny limits deterministically exercise all 3 ShardSorter paths (including concatenated
        // multi-flush Snappy frames in a single `.part` file):
        // - shard 0 (200 entries) -> multi-flush .part -> multi-run (> max_merge_fan_in=3) -> recursive merge
        // - shard 100_000 (2 entries, flushed once) -> single .part file fast path
        // - shard 50_000 (1 entry after flush) -> pure in-memory fast path
        let limits = ShardSortLimits {
            range_blocks: BODY_RANGE_BLOCKS,
            flush_bytes: 512,
            shard_mem_bytes: 1_024,
            max_merge_fan_in: 3,
            disk_sort_concurrency: 2,
        };
        let mut sorter = ShardSorter::new(&pool, &sst_root, COLUMN_BLOCK_EXT, limits);

        let mut expected = Vec::new();
        for i in (0u32..200).rev() {
            let mut hash = [0u8; BLOCK_HASH_SIZE];
            hash[..4].copy_from_slice(&i.to_be_bytes());
            let key = block_key_bytes(u64::from(i), &hash);
            let value = vec![(i % 251) as u8; 64].into_boxed_slice();
            expected.push((key, value.clone()));
            sorter.push(u64::from(i), key, value).unwrap();
        }
        for i in 0u32..2 {
            let mut hash = [0u8; BLOCK_HASH_SIZE];
            hash[0] = (2 - i) as u8;
            let num = 100_000 + u64::from(i);
            let key = block_key_bytes(num, &hash);
            let value = vec![0x5Au8; 32].into_boxed_slice();
            expected.push((key, value.clone()));
            sorter.push(num, key, value).unwrap();
        }
        sorter.flush_to_partitions().unwrap();
        let key = block_key_bytes(50_000, &[0x77u8; BLOCK_HASH_SIZE]);
        let value = vec![0x33u8; 32].into_boxed_slice();
        expected.push((key, value.clone()));
        sorter.push(50_000, key, value).unwrap();

        sorter.finish(&sst_root, &ingestor, &sst_options).unwrap();

        assert_eq!(ingestor.entries(), expected.len() as u64);
        for (key, value) in &expected {
            assert_eq!(
                target.get_pinned(COLUMN_BLOCK_EXT, key).unwrap().as_deref(),
                Some(value.as_ref())
            );
        }
    }

    #[test]
    fn block_body_batch_roundtrips_txs() {
        let hash = packed::Byte32::new([0xAB; 32]);
        let mut pending = PendingBlockBody::new();
        pending.reset([0xAB; 32], Some(7));
        pending.push_tx(0, b"first").unwrap();
        pending.push_tx(3, &[0u8; 300]).unwrap();
        let batch = pending.encode_batch().unwrap();
        assert_eq!(&batch[..BLOCK_BODY_BATCH_HEADER_BYTES], &2u32.to_be_bytes());
        assert_eq!(
            batch.len(),
            BLOCK_BODY_BATCH_HEADER_BYTES
                + (BLOCK_BODY_TX_HEADER_BYTES + 5)
                + (BLOCK_BODY_TX_HEADER_BYTES + 300)
        );

        let block_key = block_key_bytes(7, &[0xAB; 32]);
        let mut key_buf = [0u8; BLOCK_BODY_KEY_SIZE];
        let mut expanded = Vec::new();
        expand_block_body_batch(&block_key, &batch, &mut key_buf, |key, value| {
            expanded.push((key.to_vec(), value.to_vec()));
            Ok(())
        })
        .unwrap();

        assert_eq!(expanded.len(), 2);
        assert_eq!(
            expanded[0].0.as_slice(),
            block_body_key(7, &hash, 0).as_slice()
        );
        assert_eq!(expanded[0].1.as_slice(), b"first".as_slice());
        assert_eq!(
            expanded[1].0.as_slice(),
            block_body_key(7, &hash, 3).as_slice()
        );
        assert_eq!(expanded[1].1, vec![0u8; 300]);
    }

    #[test]
    fn block_body_batch_rejects_malformed_values() {
        assert!(
            expand_test_batch(&[])
                .unwrap_err()
                .to_string()
                .contains("too short")
        );
        assert!(
            expand_test_batch(&[0, 0, 0])
                .unwrap_err()
                .to_string()
                .contains("too short")
        );
        assert!(
            expand_test_batch(&0u32.to_be_bytes())
                .unwrap_err()
                .to_string()
                .contains("no transactions")
        );

        let mut truncated = Vec::new();
        truncated.extend_from_slice(&1u32.to_be_bytes());
        truncated.extend_from_slice(&0u32.to_be_bytes());
        truncated.extend_from_slice(&5u32.to_be_bytes());
        assert!(
            expand_test_batch(&truncated)
                .unwrap_err()
                .to_string()
                .contains("truncated")
        );

        let mut trailing = Vec::new();
        trailing.extend_from_slice(&1u32.to_be_bytes());
        trailing.extend_from_slice(&0u32.to_be_bytes());
        trailing.extend_from_slice(&0u32.to_be_bytes());
        trailing.push(0);
        assert!(
            expand_test_batch(&trailing)
                .unwrap_err()
                .to_string()
                .contains("trailing")
        );

        let mut non_increasing = Vec::new();
        non_increasing.extend_from_slice(&2u32.to_be_bytes());
        for _ in 0..2 {
            non_increasing.extend_from_slice(&1u32.to_be_bytes());
            non_increasing.extend_from_slice(&0u32.to_be_bytes());
        }
        assert!(
            expand_test_batch(&non_increasing)
                .unwrap_err()
                .to_string()
                .contains("non-increasing")
        );
    }

    #[test]
    fn block_body_spill_merge_expands_and_orders_across_runs() {
        fn push_block(
            sorter: &mut ShardSorter<'_>,
            number: BlockNumber,
            hash_byte: u8,
            txs: &[(u32, &[u8])],
        ) {
            let mut pending = PendingBlockBody::new();
            pending.reset([hash_byte; 32], Some(number));
            for (index, value) in txs {
                pending.push_tx(*index, value).unwrap();
            }
            let key = block_key_bytes(number, &[hash_byte; 32]);
            sorter
                .push(number, key, pending.encode_batch().unwrap())
                .unwrap();
        }

        let dir = tempfile::tempdir().unwrap();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .unwrap();
        let limits = ShardSortLimits {
            range_blocks: BODY_RANGE_BLOCKS,
            flush_bytes: 1_024,
            shard_mem_bytes: 64,
            max_merge_fan_in: 2,
            disk_sort_concurrency: 2,
        };
        let mut sorter = ShardSorter::new(&pool, dir.path(), COLUMN_BLOCK_BODY, limits);

        push_block(&mut sorter, 0, 0x02, &[(0, b"a0"), (1, b"a1")]);
        sorter.flush_to_partitions().unwrap();
        push_block(&mut sorter, 1, 0x09, &[(0, b"c1")]);
        push_block(&mut sorter, 1, 0x01, &[(0, b"b0"), (1, b"b1")]);
        push_block(&mut sorter, 50_000, 0x00, &[(0, b"d0")]);

        let target =
            RocksDB::create_for_bulk_load_open(dir.path().join("migrated"), Schema::V1).unwrap();
        let ingestor = SstIngestor::new(&target);
        let sst_options = SstWriterOptions::load();
        sorter
            .finish_with_emitter(dir.path(), &ingestor, &sst_options, make_block_body_emitter)
            .unwrap();
        assert_eq!(ingestor.entries(), 6);

        let mut actual = Vec::new();
        for (key, value) in target.iter(COLUMN_BLOCK_BODY, IteratorMode::Start).unwrap() {
            actual.push((key.to_vec(), value.to_vec()));
        }

        let expected = [
            ((0u64, 0x02u8, 0u32), b"a0".as_slice()),
            ((0, 0x02, 1), b"a1".as_slice()),
            ((1, 0x01, 0), b"b0".as_slice()),
            ((1, 0x01, 1), b"b1".as_slice()),
            ((1, 0x09, 0), b"c1".as_slice()),
            ((50_000, 0x00, 0), b"d0".as_slice()),
        ];
        assert_eq!(actual.len(), expected.len());
        for (i, ((number, hash_byte, index), value)) in expected.iter().enumerate() {
            let key = block_body_key(*number, &packed::Byte32::new([*hash_byte; 32]), *index);
            assert_eq!(actual[i].0.as_slice(), key.as_slice());
            assert_eq!(actual[i].1.as_slice(), *value);
        }
    }
}
