use crate::migrations::SST_REBUILD_VERSION;
use ckb_db::internal::{Options, SstFileWriter};
use ckb_db::{DBIterator, IteratorMode, ReadOnlyDB, RocksDB};
use ckb_db_schema::{
    CHAIN_SPEC_HASH_KEY, COLUMN_BLOCK_BODY, COLUMN_BLOCK_EPOCH, COLUMN_BLOCK_EXT,
    COLUMN_BLOCK_EXTENSION, COLUMN_BLOCK_FILTER, COLUMN_BLOCK_FILTER_HASH, COLUMN_BLOCK_HEADER,
    COLUMN_BLOCK_PROPOSAL_IDS, COLUMN_BLOCK_UNCLE, COLUMN_CELL, COLUMN_CELL_DATA,
    COLUMN_CELL_DATA_HASH, COLUMN_CHAIN_ROOT_MMR, COLUMN_EPOCH, COLUMN_HASH_INDEX, COLUMN_INDEX,
    COLUMN_META, COLUMN_NUMBER_HASH, COLUMN_TRANSACTION_INFO, COLUMN_UNCLES, COLUMNS, Col,
    META_TIP_HEADER_KEY, MIGRATION_VERSION_KEY, legacy,
};
use ckb_error::{Error, InternalErrorKind};
use ckb_logger::info;
use ckb_types::{block_number_to_key, core::BlockNumber, packed, prelude::*};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

const RANGE_BLOCKS: u64 = 50_000;
const REWRITE_SPILL_FLUSH_BYTES: usize = 256 * 1024 * 1024;
const COPY_SST_ENTRY_LIMIT: u64 = 1_000_000;
const COPY_SST_BYTES_LIMIT: usize = 256 * 1024 * 1024;
const COPY_COLUMN_PARALLELISM: usize = 4;

#[derive(Copy, Clone)]
struct ColumnMapping {
    old: Col,
    new: Col,
}

const BLOCK_KEY_COLUMNS: &[ColumnMapping] = &[
    ColumnMapping {
        old: legacy::COLUMN_BLOCK_HEADER,
        new: COLUMN_BLOCK_HEADER,
    },
    ColumnMapping {
        old: legacy::COLUMN_BLOCK_UNCLE,
        new: COLUMN_BLOCK_UNCLE,
    },
    ColumnMapping {
        old: legacy::COLUMN_BLOCK_EXT,
        new: COLUMN_BLOCK_EXT,
    },
    ColumnMapping {
        old: legacy::COLUMN_BLOCK_PROPOSAL_IDS,
        new: COLUMN_BLOCK_PROPOSAL_IDS,
    },
    ColumnMapping {
        old: legacy::COLUMN_BLOCK_EPOCH,
        new: COLUMN_BLOCK_EPOCH,
    },
    ColumnMapping {
        old: legacy::COLUMN_BLOCK_EXTENSION,
        new: COLUMN_BLOCK_EXTENSION,
    },
    ColumnMapping {
        old: legacy::COLUMN_BLOCK_FILTER,
        new: COLUMN_BLOCK_FILTER,
    },
    ColumnMapping {
        old: legacy::COLUMN_BLOCK_FILTER_HASH,
        new: COLUMN_BLOCK_FILTER_HASH,
    },
];

const COPY_COLUMNS: &[ColumnMapping] = &[
    ColumnMapping {
        old: legacy::COLUMN_TRANSACTION_INFO,
        new: COLUMN_TRANSACTION_INFO,
    },
    ColumnMapping {
        old: legacy::COLUMN_CHAIN_ROOT_MMR,
        new: COLUMN_CHAIN_ROOT_MMR,
    },
    ColumnMapping {
        old: legacy::COLUMN_META,
        new: COLUMN_META,
    },
    ColumnMapping {
        old: legacy::COLUMN_EPOCH,
        new: COLUMN_EPOCH,
    },
    ColumnMapping {
        old: legacy::COLUMN_CELL,
        new: COLUMN_CELL,
    },
    ColumnMapping {
        old: legacy::COLUMN_UNCLES,
        new: COLUMN_UNCLES,
    },
    ColumnMapping {
        old: legacy::COLUMN_CELL_DATA,
        new: COLUMN_CELL_DATA,
    },
    ColumnMapping {
        old: legacy::COLUMN_CELL_DATA_HASH,
        new: COLUMN_CELL_DATA_HASH,
    },
];

fn internal_error(reason: impl fmt::Display) -> Error {
    InternalErrorKind::Database.other(reason.to_string()).into()
}

#[derive(Clone)]
struct RebuildPlan {
    canonical_hashes: Vec<packed::Byte32>,
    forks_by_height: BTreeMap<BlockNumber, Vec<packed::Byte32>>,
    block_numbers: HashMap<[u8; 32], BlockNumber>,
    tip_number: BlockNumber,
    tip_hash: packed::Byte32,
}

impl RebuildPlan {
    fn canonical_hash(&self, number: BlockNumber) -> Result<&packed::Byte32, Error> {
        self.canonical_hashes
            .get(number as usize)
            .ok_or_else(|| internal_error(format!("missing canonical hash at height {number}")))
    }

    fn block_number_by_hash(&self, hash: &[u8]) -> Result<BlockNumber, Error> {
        let key = hash_key_from_slice(hash, "block hash")?;
        self.block_numbers
            .get(&key)
            .copied()
            .ok_or_else(|| internal_error("missing block number for block hash"))
    }
}

struct GeneratedSst {
    col: Col,
    path: PathBuf,
    entries: u64,
}

#[derive(Eq, PartialEq)]
struct BufferedEntry {
    key: Vec<u8>,
    value: Vec<u8>,
}

impl Ord for BufferedEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key
            .cmp(&other.key)
            .then_with(|| self.value.cmp(&other.value))
    }
}

impl PartialOrd for BufferedEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

struct SpillBuffer {
    entries: Vec<BufferedEntry>,
    bytes: usize,
}

struct SpillShard {
    start: BlockNumber,
    path: PathBuf,
}

struct RewriteSpill {
    col: Col,
    dir: PathBuf,
    buffers: BTreeMap<BlockNumber, SpillBuffer>,
    paths: BTreeMap<BlockNumber, PathBuf>,
    buffered_bytes: usize,
}

struct SpillRecordIter {
    path: PathBuf,
    reader: BufReader<File>,
}

pub struct SstRebuild {
    old_path: PathBuf,
    migrating_path: PathBuf,
    backup_path: PathBuf,
    sst_path: PathBuf,
    jobs: usize,
}

impl SstRebuild {
    pub fn new(old_path: PathBuf) -> Result<Self, Error> {
        let parent = old_path.parent().unwrap_or_else(|| Path::new("."));
        let db_name = old_path.file_name().unwrap_or_else(|| OsStr::new("db"));
        let db_name = db_name.to_string_lossy();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| internal_error(format!("system time error: {err}")))?
            .as_secs();

        Ok(Self {
            migrating_path: parent.join(format!("{db_name}.migrating")),
            backup_path: parent.join(format!("{db_name}.pre-sst-rebuild-{timestamp}")),
            sst_path: parent.join(format!("{db_name}.sst-rebuild-{timestamp}")),
            old_path,
            jobs: num_cpus::get().max(1),
        })
    }

    pub fn run(self) -> Result<(), Error> {
        self.prepare_paths()?;

        info!(
            "SST rebuild: starting offline migration\n  source: {}\n  target: {}\n  backup: {}\n  sst staging: {}\n  workers: {}",
            self.old_path.display(),
            self.migrating_path.display(),
            self.backup_path.display(),
            self.sst_path.display(),
            self.jobs,
        );

        info!("SST rebuild: opening old database read-only");
        let source = open_old_db(&self.old_path)?;
        ensure_not_already_rebuilt(&source)?;

        info!("SST rebuild: building migration plan");
        let plan = Arc::new(build_plan(&source)?);
        let fork_count: usize = plan.forks_by_height.values().map(Vec::len).sum();
        info!(
            "SST rebuild: plan ready, tip={}, canonical_blocks={}, fork_blocks={}, indexed_blocks={}",
            plan.tip_number,
            plan.canonical_hashes.len(),
            fork_count,
            plan.block_numbers.len(),
        );

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(self.jobs)
            .build()
            .map_err(|err| {
                internal_error(format!("failed to create rebuild thread pool: {err}"))
            })?;

        let generated = generate_ssts(&source, &plan, &self.sst_path, &pool)?;
        info!(
            "SST rebuild: generated {} SST files with {} entries",
            generated.len(),
            generated.iter().map(|sst| sst.entries).sum::<u64>(),
        );

        info!("SST rebuild: opening target database for bulk load");
        let target = RocksDB::prepare_for_bulk_load_open(&self.migrating_path, COLUMNS)?
            .ok_or_else(|| {
                internal_error(format!(
                    "failed to create target DB {}",
                    self.migrating_path.display()
                ))
            })?;
        info!("SST rebuild: ingesting generated SST files");
        ingest_ssts(&target, generated)?;
        info!("SST rebuild: copying default-column metadata");
        copy_default_keys(&source, &target)?;
        info!("SST rebuild: validating migrated database");
        validate_target(&source, &target, &plan)?;
        info!("SST rebuild: writing migration version {SST_REBUILD_VERSION}");
        target
            .put_default(MIGRATION_VERSION_KEY, SST_REBUILD_VERSION)
            .map_err(|err| internal_error(format!("failed to write migration version: {err}")))?;
        drop(target);
        drop(source);
        drop(plan);

        info!("SST rebuild: removing SST staging directory");
        let _ = fs::remove_dir_all(&self.sst_path);
        info!("SST rebuild: moving old database to backup");
        fs::rename(&self.old_path, &self.backup_path).map_err(|err| {
            internal_error(format!(
                "failed to move old DB {} to backup {}: {err}",
                self.old_path.display(),
                self.backup_path.display()
            ))
        })?;
        info!("SST rebuild: promoting migrated database");
        if let Err(err) = fs::rename(&self.migrating_path, &self.old_path) {
            let _ = fs::rename(&self.backup_path, &self.old_path);
            return Err(internal_error(format!(
                "failed to promote migrated DB {} to {}: {err}",
                self.migrating_path.display(),
                self.old_path.display()
            )));
        }

        info!(
            "SST rebuild: completed successfully; old database backup is {}",
            self.backup_path.display(),
        );
        Ok(())
    }

    fn prepare_paths(&self) -> Result<(), Error> {
        if !self.old_path.exists() {
            return Err(internal_error(format!(
                "database path {} does not exist",
                self.old_path.display()
            )));
        }
        for path in [&self.migrating_path, &self.backup_path, &self.sst_path] {
            if path.exists() {
                return Err(internal_error(format!(
                    "migration path {} already exists; remove it before retrying",
                    path.display()
                )));
            }
        }
        Ok(())
    }
}

fn open_old_db(path: &Path) -> Result<ReadOnlyDB, Error> {
    ReadOnlyDB::open_cf(path, legacy::COLUMN_FAMILIES)?
        .ok_or_else(|| internal_error(format!("database path {} does not exist", path.display())))
}

fn ensure_not_already_rebuilt(source: &ReadOnlyDB) -> Result<(), Error> {
    if let Some(version) = source.get_pinned_default(MIGRATION_VERSION_KEY)?
        && version.as_ref() >= SST_REBUILD_VERSION.as_bytes()
    {
        return Err(internal_error(
            "database already has the SST rebuild migration version",
        ));
    }
    Ok(())
}

fn build_plan(source: &ReadOnlyDB) -> Result<RebuildPlan, Error> {
    let tip_hash = get_required(source, legacy::COLUMN_META, META_TIP_HEADER_KEY)
        .and_then(|bytes| byte32_from_slice(&bytes, "tip hash"))?;
    let tip_header = get_required(source, legacy::COLUMN_BLOCK_HEADER, tip_hash.as_slice())?;
    let tip_number = header_number(&tip_header);

    info!("SST rebuild: scanning COLUMN_INDEX through height {tip_number}");
    let expected_len = tip_number as usize + 1;
    let mut canonical_hashes = vec![packed::Byte32::zero(); expected_len];
    let mut seen_canonical_hashes = vec![false; expected_len];
    let iter = source.iter(legacy::COLUMN_INDEX, IteratorMode::Start)?;
    let mut scanned_index_entries = 0u64;
    for (key, value) in iter {
        if is_legacy_hash_index_key(&key) {
            continue;
        }
        if !is_canonical_index_key(&key) {
            return Err(internal_error(format!(
                "COLUMN_INDEX key has invalid length: {} bytes",
                key.len()
            )));
        }
        let number = block_number_from_index_key(&key)?;
        if number > tip_number {
            return Err(internal_error(format!(
                "COLUMN_INDEX contains height {number} greater than tip {tip_number}"
            )));
        }

        let index = number as usize;
        if seen_canonical_hashes[index] {
            return Err(internal_error(format!(
                "COLUMN_INDEX contains duplicate canonical hash at height {number}"
            )));
        }

        let hash = byte32_from_slice(&value, "canonical hash")?;
        canonical_hashes[index] = hash;
        seen_canonical_hashes[index] = true;
        scanned_index_entries += 1;
        if scanned_index_entries % 1_000_000 == 0 {
            info!("SST rebuild: scanned {scanned_index_entries} COLUMN_INDEX entries");
        }
    }
    if scanned_index_entries as usize != expected_len {
        let missing = seen_canonical_hashes
            .iter()
            .position(|seen| !seen)
            .unwrap_or(expected_len);
        return Err(internal_error(format!(
            "COLUMN_INDEX contains {scanned_index_entries} entries, expected {expected_len}; first missing height {missing}"
        )));
    }

    info!("SST rebuild: scanning block headers to discover fork blocks");
    let mut forks_by_height: BTreeMap<BlockNumber, Vec<packed::Byte32>> = BTreeMap::new();
    let mut block_numbers = HashMap::with_capacity(expected_len);
    let iter = source.iter(legacy::COLUMN_BLOCK_HEADER, IteratorMode::Start)?;
    let mut scanned_headers = 0u64;
    for (key, value) in iter {
        let hash = byte32_from_slice(&key, "block header key")?;
        let number = header_number(&value);
        block_numbers.insert(
            hash_key_from_slice(hash.as_slice(), "block header key")?,
            number,
        );
        if canonical_hashes
            .get(number as usize)
            .map(|canonical| canonical.as_slice() != hash.as_slice())
            .unwrap_or(true)
        {
            forks_by_height.entry(number).or_default().push(hash);
        }
        scanned_headers += 1;
        if scanned_headers % 1_000_000 == 0 {
            info!("SST rebuild: scanned {scanned_headers} block headers");
        }
    }

    for hashes in forks_by_height.values_mut() {
        hashes.sort_by(|a, b| a.as_slice().cmp(b.as_slice()));
        hashes.dedup_by(|a, b| a.as_slice() == b.as_slice());
    }

    Ok(RebuildPlan {
        canonical_hashes,
        forks_by_height,
        block_numbers,
        tip_number,
        tip_hash,
    })
}

fn is_canonical_index_key(key: &[u8]) -> bool {
    key.len() == 8
}

fn is_legacy_hash_index_key(key: &[u8]) -> bool {
    key.len() == 32
}

fn generate_ssts(
    source: &ReadOnlyDB,
    plan: &RebuildPlan,
    sst_root: &Path,
    pool: &rayon::ThreadPool,
) -> Result<Vec<GeneratedSst>, Error> {
    info!("SST rebuild: generating sorted SST files");
    let mut generated = Vec::new();

    for col in BLOCK_KEY_COLUMNS {
        generated.extend(rewrite_block_column(source, plan, sst_root, pool, *col)?);
    }

    generated.extend(rewrite_block_body(source, plan, sst_root, pool)?);
    generated.extend(rewrite_number_hash(source, sst_root, pool)?);
    generated.extend(build_hash_index(source, plan, sst_root)?);
    generated.extend(copy_canonical_index(plan, sst_root)?);

    generated.extend(copy_columns(source, sst_root, pool)?);

    Ok(generated)
}

fn rewrite_block_column(
    source: &ReadOnlyDB,
    plan: &RebuildPlan,
    sst_root: &Path,
    pool: &rayon::ThreadPool,
    col: ColumnMapping,
) -> Result<Vec<GeneratedSst>, Error> {
    info!(
        "SST rebuild: scanning old column {} sequentially for block-key rewrite into {}",
        col.old, col.new
    );
    let mut spill = RewriteSpill::new(sst_root, col.new)?;
    let iter = source.iter(col.old, IteratorMode::Start)?;
    let mut scanned = 0u64;

    for (key, value) in iter {
        let hash = byte32_from_slice(&key, "block column key")?;
        let number = plan.block_number_by_hash(hash.as_slice())?;
        spill.push(number, hash.to_block_key(number).to_vec(), value.to_vec())?;
        scanned += 1;
        if scanned % 1_000_000 == 0 {
            info!(
                "SST rebuild: scanned {scanned} entries from old column {}",
                col.old
            );
        }
    }

    let shards = spill.finish()?;
    info!(
        "SST rebuild: old column {} scan complete, entries={scanned}, shards={}",
        col.old,
        shards.len()
    );
    write_spilled_shards(pool, col.new, sst_root, shards)
}

fn rewrite_block_body(
    source: &ReadOnlyDB,
    plan: &RebuildPlan,
    sst_root: &Path,
    pool: &rayon::ThreadPool,
) -> Result<Vec<GeneratedSst>, Error> {
    info!(
        "SST rebuild: scanning old column {} sequentially for tx-key rewrite into {}",
        legacy::COLUMN_BLOCK_BODY,
        COLUMN_BLOCK_BODY
    );
    let mut spill = RewriteSpill::new(sst_root, COLUMN_BLOCK_BODY)?;
    let iter = source.iter(legacy::COLUMN_BLOCK_BODY, IteratorMode::Start)?;
    let mut scanned = 0u64;

    for (old_key, value) in iter {
        if old_key.len() < 36 {
            return Err(internal_error(format!(
                "old transaction key is too short: {} bytes",
                old_key.len()
            )));
        }
        let hash = byte32_from_slice(&old_key[..32], "transaction key block hash")?;
        let number = plan.block_number_by_hash(hash.as_slice())?;
        let index = tx_index_from_old_key(&old_key)?;
        spill.push(
            number,
            hash.to_tx_key(number, index).to_vec(),
            value.to_vec(),
        )?;
        scanned += 1;
        if scanned % 1_000_000 == 0 {
            info!(
                "SST rebuild: scanned {scanned} entries from old column {}",
                legacy::COLUMN_BLOCK_BODY
            );
        }
    }

    let shards = spill.finish()?;
    info!(
        "SST rebuild: old column {} scan complete, entries={scanned}, shards={}",
        legacy::COLUMN_BLOCK_BODY,
        shards.len()
    );
    write_spilled_shards(pool, COLUMN_BLOCK_BODY, sst_root, shards)
}

fn rewrite_number_hash(
    source: &ReadOnlyDB,
    sst_root: &Path,
    pool: &rayon::ThreadPool,
) -> Result<Vec<GeneratedSst>, Error> {
    info!(
        "SST rebuild: scanning old column {} sequentially for number-hash rewrite into {}",
        legacy::COLUMN_NUMBER_HASH,
        COLUMN_NUMBER_HASH,
    );
    let mut spill = RewriteSpill::new(sst_root, COLUMN_NUMBER_HASH)?;
    let iter = source.iter(legacy::COLUMN_NUMBER_HASH, IteratorMode::Start)?;
    let mut scanned = 0u64;

    for (old_key, value) in iter {
        if old_key.len() != 40 {
            return Err(internal_error(format!(
                "old number-hash key has invalid length: {} bytes",
                old_key.len()
            )));
        }
        let number = block_number_from_index_key(&old_key[..8])?;
        let mut new_key = Vec::with_capacity(40);
        new_key.extend_from_slice(&block_number_to_key(number));
        new_key.extend_from_slice(&old_key[8..40]);
        spill.push(number, new_key, value.to_vec())?;
        scanned += 1;
        if scanned % 1_000_000 == 0 {
            info!(
                "SST rebuild: scanned {scanned} entries from old column {}",
                legacy::COLUMN_NUMBER_HASH
            );
        }
    }

    let shards = spill.finish()?;
    info!(
        "SST rebuild: old column {} scan complete, entries={scanned}, shards={}",
        legacy::COLUMN_NUMBER_HASH,
        shards.len()
    );
    write_spilled_shards(pool, COLUMN_NUMBER_HASH, sst_root, shards)
}

fn build_hash_index(
    source: &ReadOnlyDB,
    plan: &RebuildPlan,
    sst_root: &Path,
) -> Result<Option<GeneratedSst>, Error> {
    let path = sst_file_path(sst_root, COLUMN_HASH_INDEX, "all")?;
    write_sst_file(COLUMN_HASH_INDEX, path, |writer, last_key| {
        let mut entries = 0;
        let iter = source.iter(legacy::COLUMN_BLOCK_HEADER, IteratorMode::Start)?;
        for (key, value) in iter {
            let hash = byte32_from_slice(&key, "block header key")?;
            let number = header_number(&value);
            let is_main_chain = plan
                .canonical_hashes
                .get(number as usize)
                .map(|canonical| canonical.as_slice() == hash.as_slice())
                .unwrap_or(false);
            let index_value = packed::Byte32::to_index_value(number, is_main_chain);
            put_sorted(writer, last_key, hash.as_slice(), &index_value)?;
            entries += 1;
        }
        Ok(entries)
    })
    .map(Some)
}

fn copy_column(
    source: &ReadOnlyDB,
    sst_root: &Path,
    col: ColumnMapping,
) -> Result<Vec<GeneratedSst>, Error> {
    info!(
        "SST rebuild: copying old column {} into {}",
        col.old, col.new
    );
    let opts = Options::default();
    let mut generated = Vec::new();
    let mut writer = None;
    let mut path = PathBuf::new();
    let mut last_key = None;
    let mut file_index = 0u64;
    let mut entries = 0u64;
    let mut bytes = 0usize;
    let mut total_entries = 0u64;

    let iter = source.iter(col.old, IteratorMode::Start)?;
    for (key, value) in iter {
        if writer.is_some() && (entries >= COPY_SST_ENTRY_LIMIT || bytes >= COPY_SST_BYTES_LIMIT) {
            let finished =
                finish_sst_writer(writer.take().unwrap(), col.new, path.clone(), entries)?;
            generated.push(finished);
            file_index += 1;
            entries = 0;
            bytes = 0;
            last_key = None;
        }

        if writer.is_none() {
            path = sst_file_path(sst_root, col.new, &format!("copy-{file_index:06}"))?;
            let new_writer = SstFileWriter::create(&opts);
            new_writer.open(&path).map_err(|err| {
                internal_error(format!("failed to open SST {}: {err}", path.display()))
            })?;
            writer = Some(new_writer);
        }

        put_sorted(writer.as_mut().unwrap(), &mut last_key, &key, &value)?;
        entries += 1;
        bytes += key.len() + value.len();
        total_entries += 1;
        if total_entries % 1_000_000 == 0 {
            info!(
                "SST rebuild: copied {total_entries} entries from old column {}",
                col.old
            );
        }
    }

    if let Some(writer) = writer {
        generated.push(finish_sst_writer(writer, col.new, path, entries)?);
    }

    info!(
        "SST rebuild: copied old column {}, entries={total_entries}, files={}",
        col.old,
        generated.len()
    );
    Ok(generated)
}

fn copy_columns(
    source: &ReadOnlyDB,
    sst_root: &Path,
    pool: &rayon::ThreadPool,
) -> Result<Vec<GeneratedSst>, Error> {
    info!(
        "SST rebuild: copying {} old columns with up to {} concurrent scans",
        COPY_COLUMNS.len(),
        COPY_COLUMN_PARALLELISM
    );

    let mut generated = Vec::new();
    for batch in COPY_COLUMNS.chunks(COPY_COLUMN_PARALLELISM) {
        let results: Vec<Result<Vec<GeneratedSst>, Error>> = pool.install(|| {
            batch
                .par_iter()
                .copied()
                .map(|col| copy_column(source, sst_root, col))
                .collect()
        });

        for result in results {
            generated.extend(result?);
        }
    }

    Ok(generated)
}

fn copy_canonical_index(
    plan: &RebuildPlan,
    sst_root: &Path,
) -> Result<Option<GeneratedSst>, Error> {
    let path = sst_file_path(sst_root, COLUMN_INDEX, "canonical")?;
    write_sst_file(COLUMN_INDEX, path, |writer, last_key| {
        let mut entries = 0;
        for (number, hash) in plan.canonical_hashes.iter().enumerate() {
            let key = block_number_to_key(number as BlockNumber);
            put_sorted(writer, last_key, &key, hash.as_slice())?;
            entries += 1;
        }
        Ok(entries)
    })
    .map(Some)
}

impl RewriteSpill {
    fn new(sst_root: &Path, col: Col) -> Result<Self, Error> {
        let dir = sst_root.join("spill").join(col);
        fs::create_dir_all(&dir).map_err(|err| {
            internal_error(format!(
                "failed to create spill directory {}: {err}",
                dir.display()
            ))
        })?;
        Ok(Self {
            col,
            dir,
            buffers: BTreeMap::new(),
            paths: BTreeMap::new(),
            buffered_bytes: 0,
        })
    }

    fn push(&mut self, number: BlockNumber, key: Vec<u8>, value: Vec<u8>) -> Result<(), Error> {
        let start = number / RANGE_BLOCKS * RANGE_BLOCKS;
        let entry_bytes = key.len() + value.len() + 8;
        let buffer = self.buffers.entry(start).or_insert_with(|| SpillBuffer {
            entries: Vec::new(),
            bytes: 0,
        });
        buffer.entries.push(BufferedEntry { key, value });
        buffer.bytes += entry_bytes;
        self.buffered_bytes += entry_bytes;

        if self.buffered_bytes >= REWRITE_SPILL_FLUSH_BYTES {
            self.flush()?;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<SpillShard>, Error> {
        self.flush()?;
        Ok(self
            .paths
            .into_iter()
            .map(|(start, path)| SpillShard { start, path })
            .collect())
    }

    fn flush(&mut self) -> Result<(), Error> {
        if self.buffered_bytes == 0 {
            return Ok(());
        }

        let buffers = std::mem::take(&mut self.buffers);
        for (start, buffer) in buffers {
            let path = self.spill_path(start);
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|err| {
                    internal_error(format!(
                        "failed to open spill file {}: {err}",
                        path.display()
                    ))
                })?;
            let mut writer = BufWriter::new(file);
            for entry in buffer.entries {
                write_spill_record(&mut writer, &entry.key, &entry.value)?;
            }
            writer.flush().map_err(|err| {
                internal_error(format!(
                    "failed to flush spill file {}: {err}",
                    path.display()
                ))
            })?;
            self.paths.insert(start, path);
        }

        info!("SST rebuild: flushed spill buffers for column {}", self.col);
        self.buffered_bytes = 0;
        Ok(())
    }

    fn spill_path(&self, start: BlockNumber) -> PathBuf {
        self.dir.join(format!("{start:016x}.spill"))
    }
}

impl SpillRecordIter {
    fn open(path: &Path) -> Result<Self, Error> {
        let file = File::open(path).map_err(|err| {
            internal_error(format!(
                "failed to open spill file {}: {err}",
                path.display()
            ))
        })?;
        Ok(Self {
            path: path.to_path_buf(),
            reader: BufReader::new(file),
        })
    }
}

impl Iterator for SpillRecordIter {
    type Item = Result<BufferedEntry, io::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        read_spill_record_io(&mut self.reader)
            .map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("failed to read spill file {}: {err}", self.path.display()),
                )
            })
            .transpose()
    }
}

fn write_spilled_shards(
    pool: &rayon::ThreadPool,
    col: Col,
    sst_root: &Path,
    shards: Vec<SpillShard>,
) -> Result<Vec<GeneratedSst>, Error> {
    if shards.is_empty() {
        return Ok(Vec::new());
    }

    let total = shards.len();
    let completed = AtomicUsize::new(0);
    let progress_interval = (total / 20).max(1);
    info!("SST rebuild: writing {total} sorted SST shards for column {col}");

    let results: Vec<Result<GeneratedSst, Error>> = pool.install(|| {
        shards
            .into_par_iter()
            .map(|shard| {
                let result = write_spilled_shard(col, sst_root, shard);
                let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                if done == total || done % progress_interval == 0 {
                    info!("SST rebuild: wrote sorted shards {done}/{total} for column {col}");
                }
                result
            })
            .collect()
    });

    let mut generated = Vec::with_capacity(results.len());
    for result in results {
        generated.push(result?);
    }
    Ok(generated)
}

fn write_spilled_shard(
    col: Col,
    sst_root: &Path,
    shard: SpillShard,
) -> Result<GeneratedSst, Error> {
    let mut entries = SpillRecordIter::open(&shard.path)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            internal_error(format!(
                "failed to read spill file {}: {err}",
                shard.path.display()
            ))
        })?;
    entries.par_sort_unstable();

    let end = shard.start + RANGE_BLOCKS;
    let path = sst_file_path(sst_root, col, &format!("{:016x}-{end:016x}", shard.start))?;
    let generated = write_sst_file(col, path, |writer, last_key| {
        let count = entries.len() as u64;
        for entry in entries {
            put_sorted(writer, last_key, &entry.key, &entry.value)?;
        }
        Ok(count)
    })?;

    let _ = fs::remove_file(&shard.path);
    Ok(generated)
}

fn write_spill_record(writer: &mut BufWriter<File>, key: &[u8], value: &[u8]) -> Result<(), Error> {
    write_spill_record_io(writer, key, value)
        .map_err(|err| internal_error(format!("failed to write spill record: {err}")))
}

fn write_spill_record_io(writer: &mut impl Write, key: &[u8], value: &[u8]) -> io::Result<()> {
    let key_len = u32::try_from(key.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "spill key too large"))?;
    let value_len = u32::try_from(value.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "spill value too large"))?;
    writer
        .write_all(&key_len.to_le_bytes())
        .and_then(|_| writer.write_all(&value_len.to_le_bytes()))
        .and_then(|_| writer.write_all(key))
        .and_then(|_| writer.write_all(value))
}

fn read_spill_record_io(reader: &mut impl Read) -> io::Result<Option<BufferedEntry>> {
    let mut key_len_bytes = [0u8; 4];
    match reader.read_exact(&mut key_len_bytes) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }

    let mut value_len_bytes = [0u8; 4];
    reader.read_exact(&mut value_len_bytes)?;
    let key_len = u32::from_le_bytes(key_len_bytes) as usize;
    let value_len = u32::from_le_bytes(value_len_bytes) as usize;
    let mut key = vec![0u8; key_len];
    let mut value = vec![0u8; value_len];
    reader.read_exact(&mut key)?;
    reader.read_exact(&mut value)?;
    Ok(Some(BufferedEntry { key, value }))
}

fn write_sst_file<F>(col: Col, path: PathBuf, write_entries: F) -> Result<GeneratedSst, Error>
where
    F: FnOnce(&mut SstFileWriter<'_>, &mut Option<Vec<u8>>) -> Result<u64, Error>,
{
    let opts = Options::default();
    let mut writer = SstFileWriter::create(&opts);
    writer
        .open(&path)
        .map_err(|err| internal_error(format!("failed to open SST {}: {err}", path.display())))?;
    let mut last_key = None;
    let entries = write_entries(&mut writer, &mut last_key)?;
    writer
        .finish()
        .map_err(|err| internal_error(format!("failed to finish SST {}: {err}", path.display())))?;
    Ok(GeneratedSst { col, path, entries })
}

fn finish_sst_writer(
    mut writer: SstFileWriter<'_>,
    col: Col,
    path: PathBuf,
    entries: u64,
) -> Result<GeneratedSst, Error> {
    writer
        .finish()
        .map_err(|err| internal_error(format!("failed to finish SST {}: {err}", path.display())))?;
    Ok(GeneratedSst { col, path, entries })
}

fn put_sorted(
    writer: &mut SstFileWriter<'_>,
    last_key: &mut Option<Vec<u8>>,
    key: &[u8],
    value: &[u8],
) -> Result<(), Error> {
    if let Some(last) = last_key
        && key <= last.as_slice()
    {
        return Err(internal_error(
            "SST keys must be written in strictly increasing order",
        ));
    }
    writer
        .put(key, value)
        .map_err(|err| internal_error(format!("failed to write SST entry: {err}")))?;
    *last_key = Some(key.to_vec());
    Ok(())
}

fn ingest_ssts(target: &RocksDB, generated: Vec<GeneratedSst>) -> Result<(), Error> {
    let mut by_col: BTreeMap<Col, Vec<PathBuf>> = BTreeMap::new();
    for sst in generated {
        if sst.entries > 0 {
            by_col.entry(sst.col).or_default().push(sst.path);
        }
    }

    for (col, mut paths) in by_col {
        paths.sort();
        info!("SST rebuild: ingesting column {col}, files={}", paths.len());
        target.ingest_external_files(col, paths).map_err(|err| {
            internal_error(format!("failed to ingest SSTs for column {col}: {err}"))
        })?;
        info!("SST rebuild: finished ingesting column {col}");
    }
    Ok(())
}

fn copy_default_keys(source: &ReadOnlyDB, target: &RocksDB) -> Result<(), Error> {
    if let Some(value) = source.get_pinned_default(CHAIN_SPEC_HASH_KEY)? {
        target
            .put_default(CHAIN_SPEC_HASH_KEY, value.as_ref())
            .map_err(|err| internal_error(format!("failed to copy chain spec hash: {err}")))?;
    }
    Ok(())
}

fn validate_target(source: &ReadOnlyDB, target: &RocksDB, plan: &RebuildPlan) -> Result<(), Error> {
    let old_tip = get_required(source, legacy::COLUMN_META, META_TIP_HEADER_KEY)?;
    let new_tip = target
        .get_pinned(COLUMN_META, META_TIP_HEADER_KEY)?
        .ok_or_else(|| internal_error("migrated DB is missing tip header metadata"))?;
    if old_tip.as_slice() != new_tip.as_ref() {
        return Err(internal_error("migrated DB tip hash does not match old DB"));
    }

    for number in sample_heights(plan.tip_number) {
        let hash = plan.canonical_hash(number)?;
        let block_key = hash.to_block_key(number);
        if target
            .get_pinned(COLUMN_BLOCK_HEADER, &block_key)?
            .is_none()
        {
            return Err(internal_error(format!(
                "migrated DB is missing canonical header at height {number}"
            )));
        }
        let indexed_hash = target
            .get_pinned(COLUMN_INDEX, &block_number_to_key(number))?
            .ok_or_else(|| internal_error("migrated DB is missing canonical number index"))?;
        if indexed_hash.as_ref() != hash.as_slice() {
            return Err(internal_error(format!(
                "migrated DB has invalid canonical hash at height {number}"
            )));
        }
        let index_value = target
            .get_pinned(COLUMN_HASH_INDEX, hash.as_slice())?
            .ok_or_else(|| internal_error("migrated DB is missing hash index entry"))?;
        if packed::Byte32::number_from_index_value(index_value.as_ref()) != Some(number)
            || packed::Byte32::is_main_chain_from_index_value(index_value.as_ref()) != Some(true)
        {
            return Err(internal_error(format!(
                "migrated DB has invalid hash index entry at height {number}"
            )));
        }
    }

    let tip_key = plan.tip_hash.to_block_key(plan.tip_number);
    if target.get_pinned(COLUMN_BLOCK_HEADER, &tip_key)?.is_none() {
        return Err(internal_error("migrated DB is missing tip header"));
    }
    Ok(())
}

fn sample_heights(tip_number: BlockNumber) -> Vec<BlockNumber> {
    let mut heights = vec![0, tip_number / 2, tip_number];
    heights.sort_unstable();
    heights.dedup();
    heights
}

fn sst_file_path(sst_root: &Path, col: Col, name: &str) -> Result<PathBuf, Error> {
    let dir = sst_root.join(col);
    fs::create_dir_all(&dir).map_err(|err| {
        internal_error(format!(
            "failed to create SST column directory {}: {err}",
            dir.display()
        ))
    })?;
    Ok(dir.join(format!("{name}.sst")))
}

fn get_optional(source: &ReadOnlyDB, col: Col, key: &[u8]) -> Result<Option<Vec<u8>>, Error> {
    source
        .get_pinned(col, key)
        .map(|value| value.map(|slice| slice.to_vec()))
}

fn get_required(source: &ReadOnlyDB, col: Col, key: &[u8]) -> Result<Vec<u8>, Error> {
    get_optional(source, col, key)?.ok_or_else(|| {
        internal_error(format!(
            "required key with {} bytes missing from column {col}",
            key.len()
        ))
    })
}

fn byte32_from_slice(slice: &[u8], label: &str) -> Result<packed::Byte32, Error> {
    packed::Byte32::from_slice(slice)
        .map_err(|err| internal_error(format!("invalid {label}: {err}")))
}

fn hash_key_from_slice(slice: &[u8], label: &str) -> Result<[u8; 32], Error> {
    slice.try_into().map_err(|_| {
        internal_error(format!(
            "invalid {label}: expected 32 bytes, actual {}",
            slice.len()
        ))
    })
}

fn header_number(value: &[u8]) -> BlockNumber {
    let reader = packed::HeaderViewReader::from_slice_should_be_ok(value);
    reader.data().raw().number().into()
}

fn block_number_from_index_key(key: &[u8]) -> Result<BlockNumber, Error> {
    let number = packed::Uint64::from_slice(key)
        .map_err(|err| internal_error(format!("invalid COLUMN_INDEX key: {err}")))?;
    Ok(number.as_reader().into())
}

fn tx_index_from_old_key(key: &[u8]) -> Result<u32, Error> {
    if key.len() < 36 {
        return Err(internal_error(format!(
            "old transaction key is too short: {} bytes",
            key.len()
        )));
    }
    Ok(u32::from_be_bytes(
        key[32..36].try_into().expect("slice len checked"),
    ))
}
