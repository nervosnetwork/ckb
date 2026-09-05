use crate::migrations::SST_REBUILD_VERSION;
use ckb_db::internal::{
    BlockBasedIndexType, BlockBasedOptions, Options, SliceTransform, SstFileWriter,
};
use ckb_db::{DBIterator, IteratorMode, ReadOnlyDB, RocksDB, Schema};
use ckb_db_schema::{
    BlockHashIndexValue, BlockIndexFlag, CHAIN_SPEC_HASH_KEY, COLUMN_BLOCK_BODY,
    COLUMN_BLOCK_EPOCH, COLUMN_BLOCK_EXT, COLUMN_BLOCK_EXTENSION, COLUMN_BLOCK_FILTER,
    COLUMN_BLOCK_FILTER_HASH, COLUMN_BLOCK_HEADER, COLUMN_BLOCK_PROPOSAL_IDS, COLUMN_BLOCK_UNCLE,
    COLUMN_CELL, COLUMN_CELL_DATA, COLUMN_CELL_DATA_HASH, COLUMN_CHAIN_ROOT_MMR, COLUMN_EPOCH,
    COLUMN_FAMILIES, COLUMN_HASH_INDEX, COLUMN_INDEX, COLUMN_META, COLUMN_TRANSACTION_INFO,
    COLUMN_UNCLES, Col, META_TIP_HEADER_KEY, MIGRATION_VERSION_KEY, block_body_key, block_key,
    block_number_key, legacy,
};
use ckb_error::{Error, InternalErrorKind};
use ckb_logger::info;
use ckb_types::{core::BlockNumber, packed, prelude::*};
use rayon::prelude::*;
use std::collections::{BTreeMap, BinaryHeap, HashMap};
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

const RANGE_BLOCKS: u64 = 50_000;
const REWRITE_SPILL_FLUSH_BYTES: usize = 256 * 1024 * 1024;
const COPY_SST_ENTRY_LIMIT: u64 = 1_000_000;
const COPY_SST_BYTES_LIMIT: usize = 128 * 1024 * 1024;
const COPY_COLUMN_PARALLELISM: usize = 4;
const SPILL_RECORD_HEADER_BYTES: usize = 8;
const MIN_LEGACY_REBUILD_VERSION: &str = "20231101000000";

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

fn apply_sst_table_options(col: Col, opts: &mut Options) {
    let mut block_opts = BlockBasedOptions::default();
    block_opts.set_ribbon_filter(10.0);
    block_opts.set_index_type(BlockBasedIndexType::TwoLevelIndexSearch);
    block_opts.set_partition_filters(true);
    block_opts.set_metadata_block_size(4096);
    block_opts.set_pin_top_level_index_and_filter(true);

    if col == COLUMN_BLOCK_BODY {
        block_opts.set_whole_key_filtering(false);
        opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(40));
    }

    opts.set_block_based_table_factory(&block_opts);
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

    fn block_number_by_hash_opt(&self, hash: &[u8]) -> Result<Option<BlockNumber>, Error> {
        let key = hash_key_from_slice(hash, "block hash")?;
        Ok(self.block_numbers.get(&key).copied())
    }
}

struct GeneratedSst {
    col: Col,
    path: PathBuf,
    entries: u64,
}

struct SstIngestor<'a> {
    target: &'a RocksDB,
    lock: Mutex<()>,
    files: AtomicUsize,
    entries: AtomicU64,
    stats: Mutex<BTreeMap<Col, ColumnIngestStats>>,
}

struct SstWriterOptions {
    by_col: BTreeMap<Col, Options>,
}

#[derive(Clone, Copy, Default)]
struct ColumnIngestStats {
    files: usize,
    entries: u64,
}

struct SstGenerator<'a> {
    source: &'a ReadOnlyDB,
    plan: &'a RebuildPlan,
    sst_root: &'a Path,
    pool: &'a rayon::ThreadPool,
    ingestor: &'a SstIngestor<'a>,
    sst_options: &'a SstWriterOptions,
}

impl<'a> SstIngestor<'a> {
    fn new(target: &'a RocksDB) -> Self {
        Self {
            target,
            lock: Mutex::new(()),
            files: AtomicUsize::new(0),
            entries: AtomicU64::new(0),
            stats: Mutex::new(BTreeMap::new()),
        }
    }

    fn ingest(&self, sst: GeneratedSst) -> Result<(), Error> {
        if sst.entries == 0 {
            return Ok(());
        }

        let col = sst.col;
        let path = sst.path;
        let entries = sst.entries;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| internal_error("SST ingest lock is poisoned"))?;
        self.target
            .ingest_external_files(col, vec![path.clone()])
            .map_err(|err| {
                internal_error(format!(
                    "failed to ingest SST {} for column {col}: {err}",
                    path.display()
                ))
            })?;
        self.files.fetch_add(1, Ordering::SeqCst);
        self.entries.fetch_add(entries, Ordering::SeqCst);
        let mut stats = self
            .stats
            .lock()
            .map_err(|_| internal_error("SST ingest stats lock is poisoned"))?;
        let stat = stats.entry(col).or_default();
        stat.files += 1;
        stat.entries += entries;
        Ok(())
    }

    fn files(&self) -> usize {
        self.files.load(Ordering::SeqCst)
    }

    fn entries(&self) -> u64 {
        self.entries.load(Ordering::SeqCst)
    }

    fn stats(&self) -> Result<BTreeMap<Col, ColumnIngestStats>, Error> {
        self.stats
            .lock()
            .map(|stats| stats.clone())
            .map_err(|_| internal_error("SST ingest stats lock is poisoned"))
    }
}

impl SstWriterOptions {
    fn load() -> Self {
        let mut by_col = BTreeMap::new();
        for col in COLUMN_FAMILIES {
            let mut opts = Options::default();
            apply_sst_table_options(col, &mut opts);
            by_col.insert(*col, opts);
        }

        Self { by_col }
    }

    fn for_col(&self, col: Col) -> Result<&Options, Error> {
        self.by_col
            .get(col)
            .ok_or_else(|| internal_error(format!("missing SST writer options for column {col}")))
    }
}

struct BufferedEntry {
    key: Vec<u8>,
    value: Vec<u8>,
}

impl PartialEq for BufferedEntry {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for BufferedEntry {}

impl Ord for BufferedEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key.cmp(&other.key)
    }
}

impl PartialOrd for BufferedEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.entry.key == other.entry.key && self.run == other.run
    }
}

impl Eq for HeapEntry {}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .entry
            .key
            .cmp(&self.entry.key)
            .then_with(|| other.run.cmp(&self.run))
    }
}

impl PartialOrd for HeapEntry {
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
    paths: Vec<PathBuf>,
}

struct RewriteSpill<'a> {
    pool: &'a rayon::ThreadPool,
    col: Col,
    dir: PathBuf,
    buffers: BTreeMap<BlockNumber, SpillBuffer>,
    paths: BTreeMap<BlockNumber, Vec<PathBuf>>,
    run_indexes: BTreeMap<BlockNumber, u64>,
    buffered_bytes: usize,
}

struct SpillRecordIter {
    path: PathBuf,
    reader: BufReader<File>,
}

struct SplitSstWriter<'a> {
    opts: &'a Options,
    col: Col,
    sst_root: &'a Path,
    name_prefix: String,
    file_index: u64,
    writer: Option<SstFileWriter<'a>>,
    path: PathBuf,
    last_key: Option<Vec<u8>>,
    entries: u64,
    bytes: usize,
    generated: Vec<GeneratedSst>,
}

struct HeapEntry {
    entry: BufferedEntry,
    run: usize,
}

/// Offline migration that rebuilds RocksDB data into the new column schema via SST ingestion.
pub struct SstRebuild {
    old_path: PathBuf,
    migrating_path: PathBuf,
    backup_path: PathBuf,
    sst_path: PathBuf,
    jobs: usize,
}

impl SstRebuild {
    /// Creates an SST rebuild migration plan rooted at the existing database path.
    pub fn new(old_path: PathBuf) -> Result<Self, Error> {
        let parent = old_path.parent().unwrap_or_else(|| Path::new("."));
        let db_name = old_path.file_name().unwrap_or_else(|| OsStr::new("db"));
        let db_name = db_name.to_string_lossy();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| internal_error(format!("system time error: {err}")))?
            .as_nanos();

        Ok(Self {
            migrating_path: parent.join(format!("{db_name}.migrating")),
            backup_path: parent.join(format!("{db_name}.pre-sst-rebuild-{timestamp}")),
            sst_path: parent.join(format!("{db_name}.sst-rebuild-{timestamp}")),
            old_path,
            jobs: num_cpus::get().max(1),
        })
    }

    /// Runs the offline SST rebuild and atomically promotes the rebuilt database on success.
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
        ensure_legacy_migrations_complete(&source)?;

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

        info!("SST rebuild: opening target database for bulk load");
        let target = RocksDB::create_for_bulk_load_open(&self.migrating_path, Schema::V1)?;
        let (expected_entries, ingested_stats) = {
            let ingestor = SstIngestor::new(&target);
            let sst_options = SstWriterOptions::load();

            let expected_entries = SstGenerator {
                source: &source,
                plan: &plan,
                sst_root: &self.sst_path,
                pool: &pool,
                ingestor: &ingestor,
                sst_options: &sst_options,
            }
            .generate()?;
            let ingested_stats = ingestor.stats()?;
            info!(
                "SST rebuild: generated {} SST files with {} entries",
                ingestor.files(),
                ingestor.entries(),
            );
            (expected_entries, ingested_stats)
        };

        info!("SST rebuild: copying default-column metadata");
        copy_default_keys(&source, &target)?;
        info!("SST rebuild: validating migrated database");
        validate_target(&source, &target, &plan, &expected_entries, &ingested_stats)?;
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
    match ReadOnlyDB::open_cf(path, legacy::COLUMN_FAMILIES) {
        Ok(Some(db)) => Ok(db),
        Ok(None) => Err(internal_error(format!(
            "database path {} does not exist",
            path.display()
        ))),
        Err(legacy_err) => {
            if let Ok(Some(current_db)) = ReadOnlyDB::open_cf(path, COLUMN_FAMILIES) {
                if let Some(version) = current_db.get_pinned_default(MIGRATION_VERSION_KEY)?
                    && version.as_ref() >= SST_REBUILD_VERSION.as_bytes()
                {
                    return Err(internal_error(
                        "database already has the SST rebuild migration version",
                    ));
                }
                return Err(internal_error(format!(
                    "database uses the current column-family schema and cannot run the legacy SST rebuild migration; original legacy open error: {legacy_err}"
                )));
            }
            Err(legacy_err)
        }
    }
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

fn ensure_legacy_migrations_complete(source: &ReadOnlyDB) -> Result<(), Error> {
    let Some(version) = source.get_pinned_default(MIGRATION_VERSION_KEY)? else {
        return Err(internal_error(
            "database must run normal legacy migrations before SST rebuild",
        ));
    };
    if version.as_ref() < MIN_LEGACY_REBUILD_VERSION.as_bytes() {
        let version = String::from_utf8_lossy(version.as_ref());
        return Err(internal_error(format!(
            "database migration version {version} is older than {MIN_LEGACY_REBUILD_VERSION}; run `ckb migrate --force` before `ckb migrate --sst-rebuild`"
        )));
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
        if scanned_index_entries.is_multiple_of(1_000_000) {
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
        if scanned_headers.is_multiple_of(1_000_000) {
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

impl SstGenerator<'_> {
    fn generate(&self) -> Result<BTreeMap<Col, u64>, Error> {
        info!("SST rebuild: generating sorted SST files");
        let mut expected_entries = BTreeMap::new();

        for col in BLOCK_KEY_COLUMNS {
            let entries = self.rewrite_block_column(*col)?;
            expected_entries.insert(col.new, entries);
        }

        expected_entries.insert(COLUMN_BLOCK_BODY, self.rewrite_block_body()?);
        expected_entries.insert(COLUMN_HASH_INDEX, self.build_hash_index()?);
        expected_entries.insert(COLUMN_INDEX, self.copy_canonical_index()?);
        for (col, entries) in self.copy_columns()? {
            expected_entries.insert(col, entries);
        }

        Ok(expected_entries)
    }

    fn rewrite_block_column(&self, col: ColumnMapping) -> Result<u64, Error> {
        info!(
            "SST rebuild: scanning old column {} sequentially for block-key rewrite into {}",
            col.old, col.new
        );
        let mut spill = RewriteSpill::new(self.sst_root, col.new, self.pool)?;
        let iter = self.source.iter(col.old, IteratorMode::Start)?;
        let mut scanned = 0u64;
        let mut migrated = 0u64;
        let mut skipped_orphans = 0u64;

        for (key, value) in iter {
            let hash = byte32_from_slice(&key, "block column key")?;
            let number = if col.old == legacy::COLUMN_BLOCK_HEADER {
                header_number(&value)
            } else {
                match self.plan.block_number_by_hash_opt(hash.as_slice())? {
                    Some(number) => number,
                    None => {
                        skipped_orphans += 1;
                        scanned += 1;
                        if scanned.is_multiple_of(1_000_000) {
                            info!(
                                "SST rebuild: scanned {scanned} entries from old column {}",
                                col.old
                            );
                        }
                        continue;
                    }
                }
            };
            spill.push(number, block_key(number, &hash).to_vec(), value.to_vec())?;
            scanned += 1;
            migrated += 1;
            if scanned.is_multiple_of(1_000_000) {
                info!(
                    "SST rebuild: scanned {scanned} entries from old column {}",
                    col.old
                );
            }
        }

        let shards = spill.finish()?;
        info!(
            "SST rebuild: old column {} scan complete, scanned={scanned}, migrated={migrated}, skipped_orphans={skipped_orphans}, shards={}",
            col.old,
            shards.len()
        );
        write_spilled_shards(
            self.pool,
            col.new,
            self.sst_root,
            shards,
            self.ingestor,
            self.sst_options,
        )?;
        Ok(migrated)
    }

    fn rewrite_block_body(&self) -> Result<u64, Error> {
        info!(
            "SST rebuild: scanning old column {} sequentially for tx-key rewrite into {}",
            legacy::COLUMN_BLOCK_BODY,
            COLUMN_BLOCK_BODY
        );
        let mut spill = RewriteSpill::new(self.sst_root, COLUMN_BLOCK_BODY, self.pool)?;
        let iter = self
            .source
            .iter(legacy::COLUMN_BLOCK_BODY, IteratorMode::Start)?;
        let mut scanned = 0u64;
        let mut migrated = 0u64;
        let mut skipped_orphans = 0u64;

        for (old_key, value) in iter {
            if old_key.len() < 36 {
                return Err(internal_error(format!(
                    "old transaction key is too short: {} bytes",
                    old_key.len()
                )));
            }
            let hash = byte32_from_slice(&old_key[..32], "transaction key block hash")?;
            let Some(number) = self.plan.block_number_by_hash_opt(hash.as_slice())? else {
                skipped_orphans += 1;
                scanned += 1;
                if scanned.is_multiple_of(1_000_000) {
                    info!(
                        "SST rebuild: scanned {scanned} entries from old column {}",
                        legacy::COLUMN_BLOCK_BODY
                    );
                }
                continue;
            };
            let index = tx_index_from_old_key(&old_key)?;
            spill.push(
                number,
                block_body_key(number, &hash, index).to_vec(),
                value.to_vec(),
            )?;
            scanned += 1;
            migrated += 1;
            if scanned.is_multiple_of(1_000_000) {
                info!(
                    "SST rebuild: scanned {scanned} entries from old column {}",
                    legacy::COLUMN_BLOCK_BODY
                );
            }
        }

        let shards = spill.finish()?;
        info!(
            "SST rebuild: old column {} scan complete, scanned={scanned}, migrated={migrated}, skipped_orphans={skipped_orphans}, shards={}",
            legacy::COLUMN_BLOCK_BODY,
            shards.len()
        );
        write_spilled_shards(
            self.pool,
            COLUMN_BLOCK_BODY,
            self.sst_root,
            shards,
            self.ingestor,
            self.sst_options,
        )?;
        Ok(migrated)
    }

    fn build_hash_index(&self) -> Result<u64, Error> {
        let mut writer =
            SplitSstWriter::new(self.sst_options, COLUMN_HASH_INDEX, self.sst_root, "all")?;
        let iter = self
            .source
            .iter(legacy::COLUMN_BLOCK_HEADER, IteratorMode::Start)?;
        let mut total_entries = 0u64;
        for (key, value) in iter {
            let hash = byte32_from_slice(&key, "block header key")?;
            let number = header_number(&value);
            let flag = if self
                .plan
                .canonical_hashes
                .get(number as usize)
                .map(|canonical| canonical.as_slice() == hash.as_slice())
                .unwrap_or(false)
            {
                BlockIndexFlag::MainChain
            } else {
                BlockIndexFlag::Fork
            };
            let index_value = BlockHashIndexValue { number, flag }.encode();
            writer.put(hash.as_slice(), &index_value)?;
            total_entries += 1;
        }
        for sst in writer.finish()? {
            self.ingestor.ingest(sst)?;
        }
        Ok(total_entries)
    }

    fn copy_column(&self, col: ColumnMapping) -> Result<(Col, u64), Error> {
        info!(
            "SST rebuild: copying old column {} into {}",
            col.old, col.new
        );
        let mut generated_files = 0usize;
        let mut total_entries = 0u64;
        let mut writer = SplitSstWriter::new(self.sst_options, col.new, self.sst_root, "copy")?;

        let iter = self.source.iter(col.old, IteratorMode::Start)?;
        for (key, value) in iter {
            writer.put(&key, &value)?;
            total_entries += 1;
            if total_entries.is_multiple_of(1_000_000) {
                info!(
                    "SST rebuild: copied {total_entries} entries from old column {}",
                    col.old
                );
            }
        }

        for sst in writer.finish()? {
            self.ingestor.ingest(sst)?;
            generated_files += 1;
        }

        info!(
            "SST rebuild: copied old column {}, entries={total_entries}, files={}",
            col.old, generated_files
        );
        Ok((col.new, total_entries))
    }

    fn copy_columns(&self) -> Result<Vec<(Col, u64)>, Error> {
        info!(
            "SST rebuild: copying {} old columns with up to {} concurrent scans",
            COPY_COLUMNS.len(),
            COPY_COLUMN_PARALLELISM
        );

        let mut copied = Vec::with_capacity(COPY_COLUMNS.len());
        for batch in COPY_COLUMNS.chunks(COPY_COLUMN_PARALLELISM) {
            let results: Vec<Result<(Col, u64), Error>> = self.pool.install(|| {
                batch
                    .par_iter()
                    .copied()
                    .map(|col| self.copy_column(col))
                    .collect()
            });

            for result in results {
                copied.push(result?);
            }
        }

        Ok(copied)
    }

    fn copy_canonical_index(&self) -> Result<u64, Error> {
        let mut writer =
            SplitSstWriter::new(self.sst_options, COLUMN_INDEX, self.sst_root, "canonical")?;
        for (number, hash) in self.plan.canonical_hashes.iter().enumerate() {
            let key = block_number_key(number as BlockNumber);
            writer.put(&key, hash.as_slice())?;
        }
        for sst in writer.finish()? {
            self.ingestor.ingest(sst)?;
        }
        Ok(self.plan.canonical_hashes.len() as u64)
    }
}

impl<'a> RewriteSpill<'a> {
    fn new(sst_root: &Path, col: Col, pool: &'a rayon::ThreadPool) -> Result<Self, Error> {
        let dir = sst_root.join("spill").join(col);
        fs::create_dir_all(&dir).map_err(|err| {
            internal_error(format!(
                "failed to create spill directory {}: {err}",
                dir.display()
            ))
        })?;
        Ok(Self {
            pool,
            col,
            dir,
            buffers: BTreeMap::new(),
            paths: BTreeMap::new(),
            run_indexes: BTreeMap::new(),
            buffered_bytes: 0,
        })
    }

    fn push(&mut self, number: BlockNumber, key: Vec<u8>, value: Vec<u8>) -> Result<(), Error> {
        let start = number / RANGE_BLOCKS * RANGE_BLOCKS;
        let entry_bytes = key.len() + value.len() + SPILL_RECORD_HEADER_BYTES;
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
            .map(|(start, paths)| SpillShard { start, paths })
            .collect())
    }

    fn flush(&mut self) -> Result<(), Error> {
        if self.buffered_bytes == 0 {
            return Ok(());
        }

        let buffers = std::mem::take(&mut self.buffers);
        for (start, mut buffer) in buffers {
            self.pool.install(|| buffer.entries.par_sort_unstable());
            let path = self.spill_path(start);
            let file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
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
            self.paths.entry(start).or_default().push(path);
        }

        info!("SST rebuild: flushed spill buffers for column {}", self.col);
        self.buffered_bytes = 0;
        Ok(())
    }

    fn spill_path(&mut self, start: BlockNumber) -> PathBuf {
        let run = self.run_indexes.get(&start).copied().unwrap_or(0);
        self.run_indexes.insert(start, run + 1);
        self.dir.join(format!("{start:016x}-{run:06}.spill"))
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

impl<'a> SplitSstWriter<'a> {
    fn new(
        sst_options: &'a SstWriterOptions,
        col: Col,
        sst_root: &'a Path,
        name_prefix: impl Into<String>,
    ) -> Result<Self, Error> {
        Ok(Self {
            opts: sst_options.for_col(col)?,
            col,
            sst_root,
            name_prefix: name_prefix.into(),
            file_index: 0,
            writer: None,
            path: PathBuf::new(),
            last_key: None,
            entries: 0,
            bytes: 0,
            generated: Vec::new(),
        })
    }

    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), Error> {
        if self.writer.is_some()
            && self.entries > 0
            && (self.entries >= COPY_SST_ENTRY_LIMIT || self.bytes >= COPY_SST_BYTES_LIMIT)
        {
            self.finish_current()?;
        }

        if self.writer.is_none() {
            self.open_next()?;
        }

        put_sorted(
            self.writer.as_mut().expect("writer opened"),
            &mut self.last_key,
            key,
            value,
        )?;
        self.entries += 1;
        self.bytes += key.len() + value.len();
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<GeneratedSst>, Error> {
        self.finish_current()?;
        Ok(self.generated)
    }

    fn open_next(&mut self) -> Result<(), Error> {
        self.path = sst_file_path(
            self.sst_root,
            self.col,
            &format!("{}-{:06}", self.name_prefix, self.file_index),
        )?;
        let writer = SstFileWriter::create(self.opts);
        writer.open(&self.path).map_err(|err| {
            internal_error(format!("failed to open SST {}: {err}", self.path.display()))
        })?;
        self.writer = Some(writer);
        Ok(())
    }

    fn finish_current(&mut self) -> Result<(), Error> {
        if let Some(writer) = self.writer.take() {
            let generated = finish_sst_writer(writer, self.col, self.path.clone(), self.entries)?;
            if generated.entries > 0 {
                self.generated.push(generated);
            }
            self.file_index += 1;
            self.path = PathBuf::new();
            self.last_key = None;
            self.entries = 0;
            self.bytes = 0;
        }
        Ok(())
    }
}

fn write_spilled_shards(
    pool: &rayon::ThreadPool,
    col: Col,
    sst_root: &Path,
    shards: Vec<SpillShard>,
    ingestor: &SstIngestor<'_>,
    sst_options: &SstWriterOptions,
) -> Result<(), Error> {
    if shards.is_empty() {
        return Ok(());
    }

    let total = shards.len();
    let completed = AtomicUsize::new(0);
    let progress_interval = (total / 20).max(1);
    info!("SST rebuild: writing {total} sorted SST shards for column {col}");

    let results: Vec<Result<(), Error>> = pool.install(|| {
        shards
            .into_par_iter()
            .map(|shard| {
                let result = write_spilled_shard(col, sst_root, shard, sst_options)
                    .and_then(|ssts| {
                        for sst in ssts {
                            ingestor.ingest(sst)?;
                        }
                        Ok(())
                    });
                let done = completed.fetch_add(1, Ordering::SeqCst) + 1;
                if done == total || done.is_multiple_of(progress_interval) {
                    info!(
                        "SST rebuild: wrote and ingested sorted shards {done}/{total} for column {col}"
                    );
                }
                result
            })
            .collect()
    });

    for result in results {
        result?;
    }
    Ok(())
}

fn write_spilled_shard(
    col: Col,
    sst_root: &Path,
    shard: SpillShard,
    sst_options: &SstWriterOptions,
) -> Result<Vec<GeneratedSst>, Error> {
    let end = shard.start + RANGE_BLOCKS;
    let mut writer = SplitSstWriter::new(
        sst_options,
        col,
        sst_root,
        format!("{:016x}-{end:016x}", shard.start),
    )?;
    let mut runs = Vec::with_capacity(shard.paths.len());
    let mut heap = BinaryHeap::new();

    for path in &shard.paths {
        let mut run = SpillRecordIter::open(path)?;
        if let Some(entry) = read_next_spill_entry(&mut run)? {
            heap.push(HeapEntry {
                entry,
                run: runs.len(),
            });
        }
        runs.push(run);
    }

    while let Some(HeapEntry { entry, run }) = heap.pop() {
        writer.put(&entry.key, &entry.value)?;
        if let Some(next) = read_next_spill_entry(&mut runs[run])? {
            heap.push(HeapEntry { entry: next, run });
        }
    }

    for path in &shard.paths {
        let _ = fs::remove_file(path);
    }

    writer.finish()
}

fn write_spill_record(writer: &mut BufWriter<File>, key: &[u8], value: &[u8]) -> Result<(), Error> {
    write_spill_record_io(writer, key, value)
        .map_err(|err| internal_error(format!("failed to write spill record: {err}")))
}

fn read_next_spill_entry(run: &mut SpillRecordIter) -> Result<Option<BufferedEntry>, Error> {
    run.next()
        .transpose()
        .map_err(|err| internal_error(format!("{err}")))
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

fn copy_default_keys(source: &ReadOnlyDB, target: &RocksDB) -> Result<(), Error> {
    if let Some(value) = source.get_pinned_default(CHAIN_SPEC_HASH_KEY)? {
        target
            .put_default(CHAIN_SPEC_HASH_KEY, value.as_ref())
            .map_err(|err| internal_error(format!("failed to copy chain spec hash: {err}")))?;
    }
    Ok(())
}

fn validate_target(
    source: &ReadOnlyDB,
    target: &RocksDB,
    plan: &RebuildPlan,
    expected_entries: &BTreeMap<Col, u64>,
    ingested_stats: &BTreeMap<Col, ColumnIngestStats>,
) -> Result<(), Error> {
    validate_ingest_counts(plan, expected_entries, ingested_stats)?;

    let old_tip = get_required(source, legacy::COLUMN_META, META_TIP_HEADER_KEY)?;
    let new_tip = target
        .get_pinned(COLUMN_META, META_TIP_HEADER_KEY)?
        .ok_or_else(|| internal_error("migrated DB is missing tip header metadata"))?;
    if old_tip.as_slice() != new_tip.as_ref() {
        return Err(internal_error("migrated DB tip hash does not match old DB"));
    }

    for number in sample_heights(plan.tip_number) {
        let hash = plan.canonical_hash(number)?;
        let block_key = block_key(number, hash);
        if target
            .get_pinned(COLUMN_BLOCK_HEADER, &block_key)?
            .is_none()
        {
            return Err(internal_error(format!(
                "migrated DB is missing canonical header at height {number}"
            )));
        }
        let indexed_hash = target
            .get_pinned(COLUMN_INDEX, &block_number_key(number))?
            .ok_or_else(|| internal_error("migrated DB is missing canonical number index"))?;
        if indexed_hash.as_ref() != hash.as_slice() {
            return Err(internal_error(format!(
                "migrated DB has invalid canonical hash at height {number}"
            )));
        }
        let index_value = target
            .get_pinned(COLUMN_HASH_INDEX, hash.as_slice())?
            .ok_or_else(|| internal_error("migrated DB is missing hash index entry"))?;
        if BlockHashIndexValue::decode(index_value.as_ref())
            != Some(BlockHashIndexValue {
                number,
                flag: BlockIndexFlag::MainChain,
            })
        {
            return Err(internal_error(format!(
                "migrated DB has invalid hash index entry at height {number}"
            )));
        }
    }

    let tip_key = block_key(plan.tip_number, &plan.tip_hash);
    if target.get_pinned(COLUMN_BLOCK_HEADER, &tip_key)?.is_none() {
        return Err(internal_error("migrated DB is missing tip header"));
    }
    validate_fork_hash_index(target, plan)?;
    Ok(())
}

fn validate_ingest_counts(
    plan: &RebuildPlan,
    expected_entries: &BTreeMap<Col, u64>,
    ingested_stats: &BTreeMap<Col, ColumnIngestStats>,
) -> Result<(), Error> {
    for col in COLUMN_FAMILIES {
        let expected = expected_entries.get(col).copied().unwrap_or(0);
        let ingested = ingested_stats.get(col).copied().unwrap_or_default();
        if ingested.entries != expected {
            return Err(internal_error(format!(
                "migrated DB column {col} ingested {} entries, expected {expected}",
                ingested.entries
            )));
        }
        if expected > 0 && ingested.files == 0 {
            return Err(internal_error(format!(
                "migrated DB column {col} ingested entries without SST files"
            )));
        }
    }

    let block_count = plan.block_numbers.len() as u64;
    let canonical_count = plan.canonical_hashes.len() as u64;
    validate_expected_count(expected_entries, COLUMN_BLOCK_HEADER, block_count)?;
    validate_expected_count(expected_entries, COLUMN_HASH_INDEX, block_count)?;
    validate_expected_count(expected_entries, COLUMN_INDEX, canonical_count)?;
    Ok(())
}

fn validate_expected_count(
    expected_entries: &BTreeMap<Col, u64>,
    col: Col,
    expected: u64,
) -> Result<(), Error> {
    let actual = expected_entries.get(col).copied().unwrap_or(0);
    if actual != expected {
        return Err(internal_error(format!(
            "migration generated {actual} entries for column {col}, expected {expected}"
        )));
    }
    Ok(())
}

fn validate_fork_hash_index(target: &RocksDB, plan: &RebuildPlan) -> Result<(), Error> {
    for (number, hashes) in &plan.forks_by_height {
        for hash in hashes {
            let block_key = block_key(*number, hash);
            if target
                .get_pinned(COLUMN_BLOCK_HEADER, &block_key)?
                .is_none()
            {
                return Err(internal_error(format!(
                    "migrated DB is missing fork header at height {number}"
                )));
            }
            let index_value = target
                .get_pinned(COLUMN_HASH_INDEX, hash.as_slice())?
                .ok_or_else(|| internal_error("migrated DB is missing fork hash index entry"))?;
            if BlockHashIndexValue::decode(index_value.as_ref())
                != Some(BlockHashIndexValue {
                    number: *number,
                    flag: BlockIndexFlag::Fork,
                })
            {
                return Err(internal_error(format!(
                    "migrated DB has invalid fork hash index entry at height {number}"
                )));
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spill_record_roundtrips_raw_value() {
        let mut bytes = Vec::new();
        write_spill_record_io(&mut bytes, b"key", b"small-value").unwrap();

        let mut cursor = io::Cursor::new(bytes);
        let entry = read_spill_record_io(&mut cursor).unwrap().unwrap();
        assert_eq!(entry.key, b"key");
        assert_eq!(entry.value, b"small-value");
        assert!(read_spill_record_io(&mut cursor).unwrap().is_none());
    }
}
