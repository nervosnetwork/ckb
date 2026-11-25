mod cutover;
mod sorter;

use crate::migrations::SST_REBUILD_VERSION;
use ckb_db::internal::{Options, SstFileWriter};
use ckb_db::{
    DBIterator, Direction, IteratorMode, ReadOnlyDB, ReadOptions, RocksDB, RocksDBSnapshot, Schema,
    apply_sst_table_options,
};
use ckb_db_schema::{
    BLOCK_BODY_KEY_SIZE, BLOCK_HASH_SIZE, BLOCK_KEY_SIZE, BLOCK_NUMBER_KEY_SIZE,
    BlockHashIndexValue, BlockIndexFlag, CHAIN_SPEC_HASH_KEY, COLUMN_BLOCK_BODY,
    COLUMN_BLOCK_EPOCH, COLUMN_BLOCK_EXT, COLUMN_BLOCK_EXTENSION, COLUMN_BLOCK_FILTER,
    COLUMN_BLOCK_FILTER_HASH, COLUMN_BLOCK_HEADER, COLUMN_BLOCK_PROPOSAL_IDS, COLUMN_BLOCK_UNCLE,
    COLUMN_CELL, COLUMN_CELL_DATA, COLUMN_CELL_DATA_HASH, COLUMN_CHAIN_ROOT_MMR, COLUMN_EPOCH,
    COLUMN_FAMILIES, COLUMN_HASH_INDEX, COLUMN_INDEX, COLUMN_META, COLUMN_TRANSACTION_INFO,
    COLUMN_UNCLES, Col, META_TIP_HEADER_KEY, MIGRATION_VERSION_KEY, block_number_key, legacy,
};
use ckb_error::{Error, InternalErrorKind};
use ckb_logger::info;
use ckb_types::{core::BlockNumber, packed, prelude::*};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::hash::{BuildHasherDefault, Hasher};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::{
    Mutex,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

pub use cutover::SstRebuildLock;
pub(crate) use cutover::promoting_marker_path;
use cutover::{
    PromotingMarker, clean_db_path, parent_dir, promote_directories, recover_interrupted_promotion,
    sync_dir,
};
use sorter::{PendingBlockBody, ShardSortLimits, ShardSorter, make_block_body_emitter};

const BODY_RANGE_BLOCKS: u64 = 50_000;
const BLOCK_COL_RANGE_BLOCKS: u64 = 1_000_000;
const BODY_SPILL_FLUSH_BYTES: usize = 256 * 1024 * 1024;
const SMALL_COL_SPILL_FLUSH_BYTES: usize = 64 * 1024 * 1024;
const SHARD_DISK_SORT_CONCURRENCY: usize = 8;
const COLUMN_SCAN_CONCURRENCY: usize = 4;
const SCAN_READAHEAD_BYTES: usize = 4 * 1024 * 1024;
const COPY_SST_ENTRY_LIMIT: u64 = 1_000_000;
const COPY_SST_BYTES_LIMIT: usize = 128 * 1024 * 1024;
const MIN_LEGACY_REBUILD_VERSION: &str = "20231101000000";
const DISK_SPACE_MARGIN_PERCENT: u64 = 10;

type BlockHashBytes = [u8; BLOCK_HASH_SIZE];
type BlockKey = [u8; BLOCK_KEY_SIZE];

/// Identity-prefix hasher for uniformly distributed 32-byte Blake2b block hashes.
#[derive(Default)]
struct BlockHashHasher(u64);

impl Hasher for BlockHashHasher {
    fn write(&mut self, bytes: &[u8]) {
        if let Ok(head) = bytes.get(..8).unwrap_or_default().try_into() {
            self.0 = u64::from_ne_bytes(head);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

type BlockNumberMap = HashMap<BlockHashBytes, BlockNumber, BuildHasherDefault<BlockHashHasher>>;

/// Block-keyed columns rewritten after `COLUMN_BLOCK_HEADER` (which is fused into plan building).
const NON_HEADER_BLOCK_KEY_COLUMNS: &[(Col, Col)] = &[
    (legacy::COLUMN_BLOCK_UNCLE, COLUMN_BLOCK_UNCLE),
    (legacy::COLUMN_BLOCK_EXT, COLUMN_BLOCK_EXT),
    (legacy::COLUMN_BLOCK_PROPOSAL_IDS, COLUMN_BLOCK_PROPOSAL_IDS),
    (legacy::COLUMN_BLOCK_EPOCH, COLUMN_BLOCK_EPOCH),
    (legacy::COLUMN_BLOCK_EXTENSION, COLUMN_BLOCK_EXTENSION),
    (legacy::COLUMN_BLOCK_FILTER, COLUMN_BLOCK_FILTER),
    (legacy::COLUMN_BLOCK_FILTER_HASH, COLUMN_BLOCK_FILTER_HASH),
];

const COPY_COLUMNS: &[(Col, Col)] = &[
    (legacy::COLUMN_TRANSACTION_INFO, COLUMN_TRANSACTION_INFO),
    (legacy::COLUMN_CHAIN_ROOT_MMR, COLUMN_CHAIN_ROOT_MMR),
    (legacy::COLUMN_META, COLUMN_META),
    (legacy::COLUMN_EPOCH, COLUMN_EPOCH),
    (legacy::COLUMN_CELL, COLUMN_CELL),
    (legacy::COLUMN_UNCLES, COLUMN_UNCLES),
    (legacy::COLUMN_CELL_DATA, COLUMN_CELL_DATA),
    (legacy::COLUMN_CELL_DATA_HASH, COLUMN_CELL_DATA_HASH),
];

fn internal_error(reason: impl fmt::Display) -> Error {
    InternalErrorKind::Database.other(reason.to_string()).into()
}

fn io_err(action: &str, path: &Path, err: io::Error) -> Error {
    internal_error(format!("failed to {action} {}: {err}", path.display()))
}

fn scan_read_options() -> ReadOptions {
    let mut ro = ReadOptions::default();
    ro.fill_cache(false);
    ro.set_readahead_size(SCAN_READAHEAD_BYTES);
    ro
}

fn check_iter_status(iter: &ckb_db::iter::DBIter<'_>, col: Col) -> Result<(), Error> {
    iter.status()
        .map_err(|err| internal_error(format!("RocksDB iterator error on column {col}: {err}")))
}

#[inline]
fn block_key_bytes(number: BlockNumber, hash: &BlockHashBytes) -> BlockKey {
    let mut key = [0u8; BLOCK_KEY_SIZE];
    key[0..BLOCK_NUMBER_KEY_SIZE].copy_from_slice(&number.to_be_bytes());
    key[BLOCK_NUMBER_KEY_SIZE..BLOCK_KEY_SIZE].copy_from_slice(hash);
    key
}

#[inline]
fn block_body_key_bytes(
    number: BlockNumber,
    hash: &BlockHashBytes,
    index: u32,
) -> [u8; BLOCK_BODY_KEY_SIZE] {
    let mut key = [0u8; BLOCK_BODY_KEY_SIZE];
    key[0..BLOCK_NUMBER_KEY_SIZE].copy_from_slice(&number.to_be_bytes());
    key[BLOCK_NUMBER_KEY_SIZE..BLOCK_KEY_SIZE].copy_from_slice(hash);
    key[BLOCK_KEY_SIZE..BLOCK_BODY_KEY_SIZE].copy_from_slice(&index.to_be_bytes());
    key
}

struct RebuildPlan {
    canonical_hashes: Vec<BlockHashBytes>,
    forks_by_height: BTreeMap<BlockNumber, Vec<BlockHashBytes>>,
    block_numbers: BlockNumberMap,
    tip_number: BlockNumber,
    tip_hash: BlockHashBytes,
}

/// Compact validation summary retained after dropping the full `RebuildPlan` maps.
struct ValidationPlan {
    sample_canonical: Vec<(BlockNumber, BlockHashBytes)>,
    forks_by_height: BTreeMap<BlockNumber, Vec<BlockHashBytes>>,
    canonical_count: u64,
    block_count: u64,
    tip_number: BlockNumber,
    tip_hash: BlockHashBytes,
}

impl RebuildPlan {
    fn into_validation_plan(self) -> Result<ValidationPlan, Error> {
        let mut sample_canonical = Vec::new();
        for number in sample_heights(self.tip_number) {
            let hash = self
                .canonical_hashes
                .get(number as usize)
                .copied()
                .ok_or_else(|| {
                    internal_error(format!("missing canonical hash at height {number}"))
                })?;
            sample_canonical.push((number, hash));
        }
        Ok(ValidationPlan {
            sample_canonical,
            forks_by_height: self.forks_by_height,
            canonical_count: self.canonical_hashes.len() as u64,
            block_count: self.block_numbers.len() as u64,
            tip_number: self.tip_number,
            tip_hash: self.tip_hash,
        })
    }
}

struct GeneratedSst {
    col: Col,
    path: PathBuf,
    entries: u64,
}

#[derive(Clone, Copy, Default)]
struct ColumnIngestStats {
    files: usize,
    entries: u64,
}

struct SstIngestor<'a> {
    target: &'a RocksDB,
    lock: Mutex<()>,
    files: AtomicUsize,
    entries: AtomicU64,
    stats: Mutex<BTreeMap<Col, ColumnIngestStats>>,
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

    fn ingest_batch(&self, col: Col, ssts: Vec<GeneratedSst>) -> Result<(), Error> {
        let mut paths = Vec::with_capacity(ssts.len());
        let mut total_entries = 0u64;
        for sst in ssts {
            if sst.entries > 0 {
                debug_assert_eq!(sst.col, col);
                paths.push(sst.path);
                total_entries += sst.entries;
            }
        }
        if paths.is_empty() {
            return Ok(());
        }

        let file_count = paths.len();
        let _guard = self
            .lock
            .lock()
            .map_err(|_| internal_error("SST ingest lock is poisoned"))?;
        self.target
            .ingest_external_files(col, paths)
            .map_err(|err| {
                internal_error(format!(
                    "failed to ingest {file_count} SST file(s) for column {col}: {err}"
                ))
            })?;
        self.files.fetch_add(file_count, Ordering::SeqCst);
        self.entries.fetch_add(total_entries, Ordering::SeqCst);
        let mut stats = self
            .stats
            .lock()
            .map_err(|_| internal_error("SST ingest stats lock is poisoned"))?;
        let stat = stats.entry(col).or_default();
        stat.files += file_count;
        stat.entries += total_entries;
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

struct SstWriterOptions {
    by_col: BTreeMap<Col, Options>,
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

// ---------------------------------------------------------------------------
// Disk space check
// ---------------------------------------------------------------------------

/// Disk space sizes relevant to the SST rebuild migration.
#[derive(Clone, Copy, Debug)]
pub struct DiskSpaceReport {
    /// Size of the existing database directory.
    pub database_size: u64,
    /// Free space required to build the new database beside the old one.
    pub required: u64,
    /// Free space available beside the database, if it could be determined.
    pub available: Option<u64>,
}

impl DiskSpaceReport {
    /// Returns `None` when the available space could not be determined.
    pub fn has_enough_space(&self) -> Option<bool> {
        self.available.map(|available| available >= self.required)
    }

    /// Returns `None` when the available space could not be determined.
    ///
    /// When this is `Some(false)` the new database cannot possibly fit and the
    /// migration must be rejected.
    pub fn has_minimum_space(&self) -> Option<bool> {
        self.available
            .map(|available| available >= self.database_size)
    }
}

/// Measures the database and the free space available beside it.
pub fn disk_space_report(path: &Path) -> Result<DiskSpaceReport, Error> {
    let path = clean_db_path(path);
    if !path.exists() {
        return Err(internal_error(format!(
            "database path {} does not exist",
            path.display()
        )));
    }
    let database_size =
        dir_size(&path).map_err(|err| io_err("measure the database size at", &path, err))?;
    Ok(DiskSpaceReport {
        database_size,
        required: required_disk_space(database_size),
        available: available_space(parent_dir(&path)),
    })
}

fn required_disk_space(database_size: u64) -> u64 {
    // The old database already occupies its space on disk. The migration only
    // needs room for the new database built beside it, which is roughly the
    // same size, plus a margin for the larger keys in the new schema.
    database_size.saturating_add(database_size.saturating_mul(DISK_SPACE_MARGIN_PERCENT) / 100)
}

fn dir_size(path: &Path) -> io::Result<u64> {
    let mut total = 0u64;
    let mut pending = vec![path.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                total = total.saturating_add(entry.metadata()?.len());
            }
        }
    }
    Ok(total)
}

#[cfg(unix)]
fn available_space(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let ret = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if ret != 0 {
        return None;
    }
    let stat = unsafe { stat.assume_init() };
    #[allow(clippy::unnecessary_cast)]
    let block_size = if stat.f_frsize > 0 {
        stat.f_frsize as u64
    } else {
        stat.f_bsize as u64
    };
    #[allow(clippy::unnecessary_cast)]
    Some((stat.f_bavail as u64).saturating_mul(block_size))
}

#[cfg(not(unix))]
fn available_space(path: &Path) -> Option<u64> {
    use sysinfo::Disks;
    let disks = Disks::new_with_refreshed_list();
    let canonical = fs::canonicalize(path).ok();
    let stripped = canonical.as_deref().map(strip_verbatim_prefix);
    stripped
        .as_deref()
        .and_then(|p| disk_available_space(&disks, p))
        .or_else(|| disk_available_space(&disks, path))
}

#[cfg(not(unix))]
fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    #[cfg(windows)]
    if let Some(s) = path.to_str()
        && let Some(stripped) = s.strip_prefix(r"\\?\")
    {
        return PathBuf::from(stripped);
    }
    path.to_path_buf()
}

#[cfg(not(unix))]
fn disk_available_space(disks: &sysinfo::Disks, path: &Path) -> Option<u64> {
    disks
        .iter()
        .filter(|disk| path.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map(|disk| disk.available_space())
}

/// Formats a byte count for user-facing output.
pub fn format_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    format!("{:.1} GiB", bytes as f64 / GIB)
}

// ---------------------------------------------------------------------------
// SstRebuild public orchestration
// ---------------------------------------------------------------------------

/// Offline migration that rebuilds RocksDB data into the new column schema via SST ingestion.
pub struct SstRebuild {
    old_path: PathBuf,
    migrating_path: PathBuf,
    backup_path: PathBuf,
    sst_path: PathBuf,
    options_file: Option<PathBuf>,
    jobs: usize,
    allow_insufficient_space: bool,
}

impl SstRebuild {
    /// Creates an SST rebuild migration plan rooted at the existing database path.
    pub fn new(old_path: PathBuf) -> Result<Self, Error> {
        let old_path = clean_db_path(&old_path);
        let Some(Component::Normal(db_name)) = old_path.components().next_back() else {
            return Err(internal_error(format!(
                "database path {} must end in a directory name for SST rebuild",
                old_path.display()
            )));
        };
        let parent = parent_dir(&old_path);
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
            options_file: None,
            jobs: num_cpus::get().max(1),
            allow_insufficient_space: false,
        })
    }

    /// Configures the RocksDB options file used by this node so preflight, source opening,
    /// and target validation enforce WAL and column-family compatibility before cutover.
    pub fn with_options_file(mut self, options_file: PathBuf) -> Self {
        self.options_file = Some(options_file);
        self
    }

    /// Allows the rebuild to continue when free space is below the recommended
    /// 1.1x database size (but still at least the database size).
    ///
    /// The caller is expected to have obtained explicit confirmation from the
    /// user before enabling this.
    pub fn allow_insufficient_space(mut self) -> Self {
        self.allow_insufficient_space = true;
        self
    }

    fn check_options_file(&self) -> Result<(), Error> {
        if let Some(ref file) = self.options_file {
            RocksDB::check_options_file_for_sst_rebuild(&self.old_path, file)?;
        }
        Ok(())
    }

    fn target_options_file(&self) -> Option<&Path> {
        self.options_file
            .as_deref()
            .filter(|file| RocksDB::validate_options_file(file, Schema::V1).is_ok())
    }

    /// Performs fast read-only preflight validation (paths, options, schema, legacy version).
    pub fn preflight(&self) -> Result<(), Error> {
        self.prepare_paths()?;
        self.check_options_file()?;
        inspect_legacy_db_ready(&self.old_path)
    }

    /// Runs the offline SST rebuild and atomically promotes the rebuilt database on success.
    pub fn run(self) -> Result<(), Error> {
        self.run_with_post_swap(|| Ok(()))
    }

    /// Runs the offline SST rebuild and invokes `post_swap` before clearing the crash-recovery
    /// cutover marker.
    pub fn run_with_post_swap<F>(self, post_swap: F) -> Result<(), Error>
    where
        F: FnOnce() -> Result<(), Error>,
    {
        let _rebuild_lock = SstRebuildLock::acquire_exclusive(&self.old_path)?;
        let marker_path = promoting_marker_path(&self.old_path);
        if marker_path.exists() {
            self.check_options_file()?;
            recover_interrupted_promotion(
                &self.old_path,
                self.options_file.as_deref(),
                self.target_options_file(),
                post_swap,
            )?;
            return Ok(());
        }

        self.prepare_paths()?;
        self.check_options_file()?;

        info!(
            "SST rebuild: starting offline migration\n  source: {}\n  target: {}\n  backup: {}\n  sst staging: {}\n  workers: {}",
            self.old_path.display(),
            self.migrating_path.display(),
            self.backup_path.display(),
            self.sst_path.display(),
            self.jobs,
        );

        info!("SST rebuild: opening old database exclusively");
        let source = open_old_db(&self.old_path, self.options_file.as_deref())?;
        let snapshot = source.get_snapshot();
        ensure_legacy_version_ready(
            snapshot
                .get_pinned_default(MIGRATION_VERSION_KEY)?
                .as_deref(),
        )?;
        self.check_disk_space()?;

        if let Err(err) = build_and_verify_target(
            &snapshot,
            &self.migrating_path,
            &self.sst_path,
            self.target_options_file(),
            self.jobs,
        ) {
            let _ = fs::remove_dir_all(&self.sst_path);
            let _ = fs::remove_dir_all(&self.migrating_path);
            return Err(err);
        }

        // Persist the promoting marker while still holding the exclusive source RocksDB lock so
        // concurrent `ckb run` refuses to open `old_path`, then close `source` cleanly on its own
        // directory before swapping directories.
        drop(snapshot);
        let marker = PromotingMarker {
            migrating_name: self
                .migrating_path
                .file_name()
                .ok_or_else(|| internal_error("invalid migrating path"))?
                .to_string_lossy()
                .into_owned(),
            backup_name: self
                .backup_path
                .file_name()
                .ok_or_else(|| internal_error("invalid backup path"))?
                .to_string_lossy()
                .into_owned(),
        };
        marker.write_atomic(&marker_path)?;
        drop(source);

        promote_directories(&self.old_path, &self.migrating_path, &self.backup_path)?;
        post_swap()?;

        let parent = parent_dir(&self.old_path);
        fs::remove_file(&marker_path)
            .map_err(|e| io_err("remove cutover marker", &marker_path, e))?;
        sync_dir(parent)?;

        info!(
            "SST rebuild: completed successfully; old database backup is {}",
            self.backup_path.display(),
        );
        Ok(())
    }

    fn check_disk_space(&self) -> Result<(), Error> {
        let report = disk_space_report(&self.old_path)?;
        let parent = parent_dir(&self.old_path);
        let available = report
            .available
            .map(format_bytes)
            .unwrap_or_else(|| "unknown".to_string());
        eprintln!(
            "SST rebuild: database {} size {}, required free space {}, available {}",
            self.old_path.display(),
            format_bytes(report.database_size),
            format_bytes(report.required),
            available,
        );
        info!(
            "SST rebuild: disk space check: database={}, required={}, available={available}",
            format_bytes(report.database_size),
            format_bytes(report.required),
        );
        if report.has_minimum_space() == Some(false) {
            return Err(internal_error(format!(
                "insufficient disk space for the SST rebuild migration: only {} is available at {}, but at least {} (the current database size) is needed to build the new database; the migration would fail. Free up space or move the data directory to a larger disk, then retry",
                format_bytes(report.available.unwrap_or_default()),
                parent.display(),
                format_bytes(report.database_size),
            )));
        }
        if report.has_enough_space() == Some(false) {
            if !self.allow_insufficient_space {
                return Err(internal_error(format!(
                    "insufficient disk space for the SST rebuild migration: the database is {}, building the new database beside the old one needs about {} free (the old database is kept as a backup), but only {} is available at {}. Free up space or move the data directory to a larger disk, then retry",
                    format_bytes(report.database_size),
                    format_bytes(report.required),
                    format_bytes(report.available.unwrap_or_default()),
                    parent.display(),
                )));
            }
            eprintln!("SST rebuild: continuing despite insufficient disk space as requested");
        }
        Ok(())
    }

    fn prepare_paths(&self) -> Result<(), Error> {
        let meta = match fs::symlink_metadata(&self.old_path) {
            Ok(meta) => meta,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(internal_error(format!(
                    "database path {} does not exist",
                    self.old_path.display()
                )));
            }
            Err(err) => return Err(io_err("inspect database path", &self.old_path, err)),
        };
        if meta.file_type().is_symlink() {
            return Err(internal_error(format!(
                "database path {} is a symlink; point [db] path directly to the target directory before running --sst-rebuild",
                self.old_path.display()
            )));
        }
        if !meta.file_type().is_dir() {
            return Err(internal_error(format!(
                "database path {} is not a directory",
                self.old_path.display()
            )));
        }

        let parent = parent_dir(&self.old_path);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let parent_meta =
                fs::metadata(parent).map_err(|e| io_err("inspect parent directory", parent, e))?;
            if meta.dev() != parent_meta.dev() {
                return Err(internal_error(format!(
                    "database path {} is on a different filesystem or mount point than its parent {}; SST rebuild requires both to share a filesystem for atomic swap",
                    self.old_path.display(),
                    parent.display()
                )));
            }
        }

        for path in [&self.migrating_path, &self.backup_path, &self.sst_path] {
            if path.exists() {
                return Err(internal_error(format!(
                    "migration path {} already exists; remove it before retrying",
                    path.display()
                )));
            }
        }

        let db_name = self
            .old_path
            .file_name()
            .unwrap_or_else(|| OsStr::new("db"))
            .to_string_lossy();
        let sst_prefix = format!("{db_name}.sst-rebuild-");
        let marker_name = format!("{db_name}.sst-rebuild-promoting");
        if let Ok(entries) = fs::read_dir(parent) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with(&sst_prefix) && name_str != marker_name {
                    return Err(internal_error(format!(
                        "stale SST staging path {} already exists; remove it before retrying",
                        entry.path().display()
                    )));
                }
            }
        }
        Ok(())
    }
}

fn inspect_legacy_db_ready(path: &Path) -> Result<(), Error> {
    match RocksDB::detect_schema(path)? {
        None => Err(internal_error(format!(
            "database path {} does not exist",
            path.display()
        ))),
        Some(Schema::V1) => {
            if let Some(current_db) = ReadOnlyDB::open_cf(path, vec![COLUMN_META])?
                && current_db
                    .get_pinned_default(MIGRATION_VERSION_KEY)?
                    .as_deref()
                    .is_some_and(|v| v >= SST_REBUILD_VERSION.as_bytes())
            {
                return Err(internal_error(
                    "database already has the SST rebuild migration version",
                ));
            }
            Err(internal_error(
                "database uses the current column-family schema and cannot run the legacy SST rebuild migration",
            ))
        }
        Some(Schema::Legacy) => {
            if let Some(ro_db) = ReadOnlyDB::open_cf(path, vec![legacy::COLUMN_META])? {
                ensure_legacy_version_ready(
                    ro_db.get_pinned_default(MIGRATION_VERSION_KEY)?.as_deref(),
                )?;
            }
            Ok(())
        }
    }
}

fn open_old_db(path: &Path, options_file: Option<&Path>) -> Result<RocksDB, Error> {
    inspect_legacy_db_ready(path)?;
    RocksDB::open_existing_for_migration_with_options(path, Schema::Legacy, options_file)?
        .ok_or_else(|| internal_error(format!("database path {} does not exist", path.display())))
}

fn ensure_legacy_version_ready(version: Option<&[u8]>) -> Result<(), Error> {
    let Some(version) = version else {
        return Err(internal_error(
            "database must run normal legacy migrations before SST rebuild",
        ));
    };
    if version >= SST_REBUILD_VERSION.as_bytes() {
        return Err(internal_error(
            "database already has the SST rebuild migration version",
        ));
    }
    if version < MIN_LEGACY_REBUILD_VERSION.as_bytes() {
        let version = String::from_utf8_lossy(version);
        return Err(internal_error(format!(
            "database migration version {version} is older than {MIN_LEGACY_REBUILD_VERSION}; run `ckb migrate --force` (which advances legacy migrations to {MIN_LEGACY_REBUILD_VERSION} before stopping at the SST rebuild step) and then run `ckb migrate --sst-rebuild`"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Target build, fused scan, and SST generation
// ---------------------------------------------------------------------------

struct SstGenerator<'a> {
    source: &'a RocksDBSnapshot,
    sst_root: &'a Path,
    pool: &'a rayon::ThreadPool,
    ingestor: &'a SstIngestor<'a>,
    sst_options: &'a SstWriterOptions,
}

fn build_and_verify_target(
    snapshot: &RocksDBSnapshot,
    migrating_path: &Path,
    sst_path: &Path,
    target_options_file: Option<&Path>,
    jobs: usize,
) -> Result<(), Error> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .build()
        .map_err(|err| internal_error(format!("failed to create rebuild thread pool: {err}")))?;

    info!("SST rebuild: opening target database for bulk load");
    let target = RocksDB::create_for_bulk_load_open(migrating_path, Schema::V1)?;
    let (validation_plan, expected_entries, ingested_stats) = {
        let ingestor = SstIngestor::new(&target);
        let sst_options = SstWriterOptions::load();
        let (validation_plan, expected_entries) = SstGenerator {
            source: snapshot,
            sst_root: sst_path,
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
        (validation_plan, expected_entries, ingested_stats)
    };

    info!("SST rebuild: copying default-column metadata");
    copy_default_keys(snapshot, &target)?;
    info!("SST rebuild: writing migration version {SST_REBUILD_VERSION}");
    target
        .put_default_sync(MIGRATION_VERSION_KEY, SST_REBUILD_VERSION)
        .map_err(|err| internal_error(format!("failed to write migration version: {err}")))?;
    target
        .flush_default()
        .map_err(|err| internal_error(format!("failed to flush default column family: {err}")))?;
    drop(target);

    info!("SST rebuild: reopening migrated database with V1 table options for validation");
    let verified_target = RocksDB::open_existing_for_validation_with_options(
        migrating_path,
        Schema::V1,
        target_options_file,
    )?;
    validate_target(
        snapshot,
        &verified_target,
        &validation_plan,
        &expected_entries,
        &ingested_stats,
    )?;
    drop(verified_target);

    sync_dir(migrating_path)?;
    info!("SST rebuild: removing SST staging directory");
    let _ = fs::remove_dir_all(sst_path);
    Ok(())
}

impl SstGenerator<'_> {
    fn generate(&self) -> Result<(ValidationPlan, BTreeMap<Col, u64>), Error> {
        info!("SST rebuild: building plan and generating sorted SST files");
        let mut expected_entries = BTreeMap::new();

        // Stage 1: Single-pass canonical index + fused block header / hash index scan.
        let (plan, index_entries, header_entries, hash_index_entries) =
            self.build_plan_and_emit_core_columns()?;
        expected_entries.insert(COLUMN_INDEX, index_entries);
        expected_entries.insert(COLUMN_BLOCK_HEADER, header_entries);
        expected_entries.insert(COLUMN_HASH_INDEX, hash_index_entries);

        // Stage 2: Dedicated full-budget scan & parallel shard sort for COLUMN_BLOCK_BODY.
        expected_entries.insert(COLUMN_BLOCK_BODY, self.rewrite_block_body(&plan)?);

        // Stage 3a: Remaining 7 block-keyed columns in bounded batches.
        for (col, entries) in self
            .run_column_batches(NON_HEADER_BLOCK_KEY_COLUMNS, |old, new| {
                self.rewrite_block_column(&plan, old, new)
            })?
        {
            expected_entries.insert(col, entries);
        }

        // Release canonical_hashes and block_numbers maps before direct-copy columns & validation.
        let validation_plan = plan.into_validation_plan()?;

        // Stage 3b: 8 direct-copy columns in bounded batches.
        for (col, entries) in
            self.run_column_batches(COPY_COLUMNS, |old, new| self.copy_column(old, new))?
        {
            expected_entries.insert(col, entries);
        }

        Ok((validation_plan, expected_entries))
    }

    fn run_column_batches<F>(&self, cols: &[(Col, Col)], work: F) -> Result<Vec<(Col, u64)>, Error>
    where
        F: Fn(Col, Col) -> Result<(Col, u64), Error> + Sync,
    {
        let mut out = Vec::with_capacity(cols.len());
        for batch in cols.chunks(COLUMN_SCAN_CONCURRENCY) {
            let results: Vec<_> = self.pool.install(|| {
                batch
                    .par_iter()
                    .copied()
                    .map(|(old, new)| work(old, new))
                    .collect()
            });
            for result in results {
                out.push(result?);
            }
        }
        Ok(out)
    }

    fn build_plan_and_emit_core_columns(&self) -> Result<(RebuildPlan, u64, u64, u64), Error> {
        let tip_hash = get_required(self.source, legacy::COLUMN_META, META_TIP_HEADER_KEY)
            .and_then(|bytes| hash_bytes_from_slice(&bytes, "tip hash"))?;
        let tip_header = get_required(self.source, legacy::COLUMN_BLOCK_HEADER, &tip_hash)?;
        let tip_number = header_number(&tip_header)?;

        info!("SST rebuild: scanning COLUMN_INDEX through height {tip_number}");
        let expected_len = usize::try_from(tip_number)
            .ok()
            .and_then(|n| n.checked_add(1))
            .ok_or_else(|| {
                internal_error(format!("tip block number {tip_number} overflows usize"))
            })?;
        let mut canonical_hashes = vec![[0u8; BLOCK_HASH_SIZE]; expected_len];
        let mut seen = vec![false; expected_len];
        let ro = scan_read_options();
        let mut iter = self
            .source
            .iter_opt(legacy::COLUMN_INDEX, IteratorMode::Start, &ro)?;
        let mut scanned_index = 0u64;
        for (key, value) in iter.by_ref() {
            if key.len() == BLOCK_HASH_SIZE {
                continue;
            }
            if key.len() != BLOCK_NUMBER_KEY_SIZE {
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
            let idx = number as usize;
            if seen[idx] {
                return Err(internal_error(format!(
                    "COLUMN_INDEX contains duplicate canonical hash at height {number}"
                )));
            }
            canonical_hashes[idx] = hash_bytes_from_slice(&value, "canonical hash")?;
            seen[idx] = true;
            scanned_index += 1;
            if scanned_index.is_multiple_of(1_000_000) {
                info!("SST rebuild: scanned {scanned_index} COLUMN_INDEX entries");
            }
        }
        check_iter_status(&iter, legacy::COLUMN_INDEX)?;
        if scanned_index as usize != expected_len {
            let missing = seen.iter().position(|s| !s).unwrap_or(expected_len);
            return Err(internal_error(format!(
                "COLUMN_INDEX contains {scanned_index} entries, expected {expected_len}; first missing height {missing}"
            )));
        }
        drop(seen);
        if canonical_hashes[tip_number as usize] != tip_hash {
            return Err(internal_error(format!(
                "COLUMN_INDEX hash at tip height {tip_number} does not match META_TIP_HEADER_KEY"
            )));
        }

        let mut index_writer =
            SplitSstWriter::new(self.sst_options, COLUMN_INDEX, self.sst_root, "canonical")?;
        for (number, hash) in canonical_hashes.iter().enumerate() {
            index_writer.put(&block_number_key(number as BlockNumber), hash)?;
        }
        self.ingestor
            .ingest_batch(COLUMN_INDEX, index_writer.finish()?)?;
        let index_entries = canonical_hashes.len() as u64;

        // Single fused scan of legacy COLUMN_BLOCK_HEADER (already sorted by 32-byte hash):
        // populates block_numbers + forks_by_height, streams COLUMN_HASH_INDEX directly to SSTs,
        // and partitions COLUMN_BLOCK_HEADER for composite-key rewrite.
        info!(
            "SST rebuild: fused scan of {} into {}, {}, and rebuild plan",
            legacy::COLUMN_BLOCK_HEADER,
            COLUMN_BLOCK_HEADER,
            COLUMN_HASH_INDEX
        );
        let mut forks_by_height: BTreeMap<BlockNumber, Vec<BlockHashBytes>> = BTreeMap::new();
        let mut block_numbers = BlockNumberMap::with_capacity_and_hasher(
            expected_len,
            BuildHasherDefault::<BlockHashHasher>::default(),
        );
        let mut hash_index_writer =
            SplitSstWriter::new(self.sst_options, COLUMN_HASH_INDEX, self.sst_root, "all")?;
        let mut header_sorter = ShardSorter::new(
            self.pool,
            self.sst_root,
            COLUMN_BLOCK_HEADER,
            ShardSortLimits::for_column(
                BLOCK_COL_RANGE_BLOCKS,
                BODY_SPILL_FLUSH_BYTES,
                SHARD_DISK_SORT_CONCURRENCY,
            ),
        );

        let mut iter =
            self.source
                .iter_opt(legacy::COLUMN_BLOCK_HEADER, IteratorMode::Start, &ro)?;
        let mut scanned_headers = 0u64;
        for (key, value) in iter.by_ref() {
            let hash = hash_bytes_from_slice(&key, "block header key")?;
            let number = header_number(&value)?;
            block_numbers.insert(hash, number);

            let flag = if canonical_hashes
                .get(number as usize)
                .is_some_and(|canonical| canonical == &hash)
            {
                BlockIndexFlag::MainChain
            } else {
                forks_by_height.entry(number).or_default().push(hash);
                BlockIndexFlag::Fork
            };
            let index_value = BlockHashIndexValue { number, flag }.encode();
            hash_index_writer.put(&hash, &index_value)?;
            header_sorter.push(number, block_key_bytes(number, &hash), value)?;

            scanned_headers += 1;
            if scanned_headers.is_multiple_of(1_000_000) {
                info!("SST rebuild: scanned {scanned_headers} block headers");
            }
        }
        check_iter_status(&iter, legacy::COLUMN_BLOCK_HEADER)?;

        for (height, canonical_hash) in canonical_hashes.iter().enumerate() {
            match block_numbers.get(canonical_hash).copied() {
                Some(header_height) if header_height == height as BlockNumber => {}
                Some(header_height) => {
                    return Err(internal_error(format!(
                        "canonical hash at height {height} points to header with mismatched number {header_height}"
                    )));
                }
                None => {
                    return Err(internal_error(format!(
                        "COLUMN_BLOCK_HEADER is missing canonical header at height {height}"
                    )));
                }
            }
        }

        for hashes in forks_by_height.values_mut() {
            hashes.sort_unstable();
            hashes.dedup();
        }

        let fork_count: usize = forks_by_height.values().map(Vec::len).sum();
        info!(
            "SST rebuild: plan ready, tip={tip_number}, canonical_blocks={}, fork_blocks={fork_count}, indexed_blocks={}",
            canonical_hashes.len(),
            block_numbers.len(),
        );

        self.ingestor
            .ingest_batch(COLUMN_HASH_INDEX, hash_index_writer.finish()?)?;
        header_sorter.finish(self.sst_root, self.ingestor, self.sst_options)?;

        let plan = RebuildPlan {
            canonical_hashes,
            forks_by_height,
            block_numbers,
            tip_number,
            tip_hash,
        };
        Ok((plan, index_entries, scanned_headers, scanned_headers))
    }

    fn rewrite_block_column(
        &self,
        plan: &RebuildPlan,
        old_col: Col,
        new_col: Col,
    ) -> Result<(Col, u64), Error> {
        info!("SST rebuild: scanning old column {old_col} into {new_col}");
        // Up to COLUMN_SCAN_CONCURRENCY columns run concurrently, so cap per-column disk sort
        // concurrency to 1 to keep global open FDs and sort RAM bounded.
        let mut sorter = ShardSorter::new(
            self.pool,
            self.sst_root,
            new_col,
            ShardSortLimits::for_column(BLOCK_COL_RANGE_BLOCKS, SMALL_COL_SPILL_FLUSH_BYTES, 1),
        );
        let ro = scan_read_options();
        let mut iter = self.source.iter_opt(old_col, IteratorMode::Start, &ro)?;
        let (mut scanned, mut migrated, mut skipped_orphans) = (0u64, 0u64, 0u64);

        for (key, value) in iter.by_ref() {
            let hash = hash_bytes_from_slice(&key, "block column key")?;
            let Some(number) = plan.block_numbers.get(&hash).copied() else {
                skipped_orphans += 1;
                scanned += 1;
                continue;
            };
            sorter.push(number, block_key_bytes(number, &hash), value)?;
            scanned += 1;
            migrated += 1;
            if scanned.is_multiple_of(1_000_000) {
                info!("SST rebuild: scanned {scanned} entries from old column {old_col}");
            }
        }
        check_iter_status(&iter, old_col)?;
        info!(
            "SST rebuild: old column {old_col} scan complete, scanned={scanned}, migrated={migrated}, skipped_orphans={skipped_orphans}"
        );
        sorter.finish(self.sst_root, self.ingestor, self.sst_options)?;
        Ok((new_col, migrated))
    }

    fn rewrite_block_body(&self, plan: &RebuildPlan) -> Result<u64, Error> {
        info!(
            "SST rebuild: scanning old column {} into {}",
            legacy::COLUMN_BLOCK_BODY,
            COLUMN_BLOCK_BODY
        );
        let mut sorter = ShardSorter::new(
            self.pool,
            self.sst_root,
            COLUMN_BLOCK_BODY,
            ShardSortLimits::for_column(
                BODY_RANGE_BLOCKS,
                BODY_SPILL_FLUSH_BYTES,
                SHARD_DISK_SORT_CONCURRENCY,
            ),
        );
        let ro = scan_read_options();
        let mut iter = self
            .source
            .iter_opt(legacy::COLUMN_BLOCK_BODY, IteratorMode::Start, &ro)?;
        let (mut scanned, mut migrated, mut skipped_orphans) = (0u64, 0u64, 0u64);
        let mut pending = PendingBlockBody::new();

        for (old_key, value) in iter.by_ref() {
            if old_key.len() != 36 {
                return Err(internal_error(format!(
                    "old transaction key has invalid length: expected 36 bytes, actual {}",
                    old_key.len()
                )));
            }
            let hash: BlockHashBytes = old_key[..BLOCK_HASH_SIZE]
                .try_into()
                .expect("slice len checked");
            if !pending.active || pending.hash != hash {
                pending.flush_into(&mut sorter, &mut migrated, &mut skipped_orphans)?;
                pending.reset(hash, plan.block_numbers.get(&hash).copied());
            }
            let index = u32::from_be_bytes(old_key[32..36].try_into().expect("slice len checked"));
            pending.push_tx(index, &value)?;

            scanned += 1;
            if scanned.is_multiple_of(1_000_000) {
                info!(
                    "SST rebuild: scanned {scanned} entries from old column {}",
                    legacy::COLUMN_BLOCK_BODY
                );
            }
        }
        check_iter_status(&iter, legacy::COLUMN_BLOCK_BODY)?;
        pending.flush_into(&mut sorter, &mut migrated, &mut skipped_orphans)?;

        info!(
            "SST rebuild: old column {} scan complete, scanned={scanned}, migrated={migrated}, skipped_orphans={skipped_orphans}",
            legacy::COLUMN_BLOCK_BODY
        );
        sorter.finish_with_emitter(
            self.sst_root,
            self.ingestor,
            self.sst_options,
            make_block_body_emitter,
        )?;
        Ok(migrated)
    }

    fn copy_column(&self, old_col: Col, new_col: Col) -> Result<(Col, u64), Error> {
        info!("SST rebuild: copying old column {old_col} into {new_col}");
        let mut total_entries = 0u64;
        let mut writer = SplitSstWriter::new(self.sst_options, new_col, self.sst_root, "copy")?;
        let ro = scan_read_options();
        let mut iter = self.source.iter_opt(old_col, IteratorMode::Start, &ro)?;
        for (key, value) in iter.by_ref() {
            writer.put(&key, &value)?;
            total_entries += 1;
            if total_entries.is_multiple_of(1_000_000) {
                info!("SST rebuild: copied {total_entries} entries from old column {old_col}");
            }
        }
        check_iter_status(&iter, old_col)?;
        let ssts = writer.finish()?;
        let files = ssts.len();
        self.ingestor.ingest_batch(new_col, ssts)?;
        info!("SST rebuild: copied old column {old_col}, entries={total_entries}, files={files}");
        Ok((new_col, total_entries))
    }
}

// ---------------------------------------------------------------------------
// SplitSstWriter
// ---------------------------------------------------------------------------

struct SplitSstWriter<'a> {
    opts: &'a Options,
    col: Col,
    sst_root: &'a Path,
    name_prefix: String,
    file_index: u64,
    writer: Option<SstFileWriter<'a>>,
    path: PathBuf,
    last_key: Vec<u8>,
    has_last_key: bool,
    entries: u64,
    bytes: usize,
    generated: Vec<GeneratedSst>,
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
            last_key: Vec::new(),
            has_last_key: false,
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
        if self.has_last_key && key <= self.last_key.as_slice() {
            return Err(internal_error(
                "SST keys must be written in strictly increasing order",
            ));
        }
        self.writer
            .as_mut()
            .expect("writer opened")
            .put(key, value)
            .map_err(|err| internal_error(format!("failed to write SST entry: {err}")))?;
        self.last_key.clear();
        self.last_key.extend_from_slice(key);
        self.has_last_key = true;
        self.entries += 1;
        self.bytes += key.len() + value.len();
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<GeneratedSst>, Error> {
        self.finish_current()?;
        Ok(self.generated)
    }

    fn open_next(&mut self) -> Result<(), Error> {
        let dir = self.sst_root.join(self.col);
        fs::create_dir_all(&dir).map_err(|e| io_err("create SST dir", &dir, e))?;
        self.path = dir.join(format!("{}-{:06}.sst", self.name_prefix, self.file_index));
        let writer = SstFileWriter::create(self.opts);
        writer.open(&self.path).map_err(|err| {
            internal_error(format!("failed to open SST {}: {err}", self.path.display()))
        })?;
        self.writer = Some(writer);
        Ok(())
    }

    fn finish_current(&mut self) -> Result<(), Error> {
        if let Some(mut writer) = self.writer.take() {
            writer.finish().map_err(|err| {
                internal_error(format!(
                    "failed to finish SST {}: {err}",
                    self.path.display()
                ))
            })?;
            if self.entries > 0 {
                self.generated.push(GeneratedSst {
                    col: self.col,
                    path: std::mem::take(&mut self.path),
                    entries: self.entries,
                });
            }
            self.file_index += 1;
            self.last_key.clear();
            self.has_last_key = false;
            self.entries = 0;
            self.bytes = 0;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Default metadata copy & target validation
// ---------------------------------------------------------------------------

fn copy_default_keys(source: &RocksDBSnapshot, target: &RocksDB) -> Result<(), Error> {
    if let Some(value) = source.get_pinned_default(CHAIN_SPEC_HASH_KEY)? {
        target
            .put_default_sync(CHAIN_SPEC_HASH_KEY, value.as_ref())
            .map_err(|err| internal_error(format!("failed to copy chain spec hash: {err}")))?;
    }
    Ok(())
}

fn validate_target(
    source: &RocksDBSnapshot,
    target: &RocksDB,
    plan: &ValidationPlan,
    expected_entries: &BTreeMap<Col, u64>,
    ingested_stats: &BTreeMap<Col, ColumnIngestStats>,
) -> Result<(), Error> {
    validate_ingest_counts(plan, expected_entries, ingested_stats)?;

    if target.get_pinned_default(MIGRATION_VERSION_KEY)?.as_deref()
        != Some(SST_REBUILD_VERSION.as_bytes())
    {
        return Err(internal_error(
            "migrated DB migration version does not match SST rebuild version",
        ));
    }

    if let Some(old_chain_spec) = source.get_pinned_default(CHAIN_SPEC_HASH_KEY)?
        && target.get_pinned_default(CHAIN_SPEC_HASH_KEY)?.as_deref()
            != Some(old_chain_spec.as_ref())
    {
        return Err(internal_error(
            "migrated DB chain spec hash does not match old DB",
        ));
    }

    let old_tip = get_required(source, legacy::COLUMN_META, META_TIP_HEADER_KEY)?;
    if target
        .get_pinned(COLUMN_META, META_TIP_HEADER_KEY)?
        .as_deref()
        != Some(old_tip.as_slice())
    {
        return Err(internal_error("migrated DB tip hash does not match old DB"));
    }

    for &(number, ref hash) in &plan.sample_canonical {
        let bk = block_key_bytes(number, hash);
        if target.get_pinned(COLUMN_BLOCK_HEADER, &bk)?.is_none() {
            return Err(internal_error(format!(
                "migrated DB is missing canonical header at height {number}"
            )));
        }
        if target
            .get_pinned(COLUMN_INDEX, &block_number_key(number))?
            .as_deref()
            != Some(hash.as_slice())
        {
            return Err(internal_error(format!(
                "migrated DB has invalid canonical hash at height {number}"
            )));
        }
        let index_value = target
            .get_pinned(COLUMN_HASH_INDEX, hash)?
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

        // Verify block body point lookup and 40-byte prefix scan against source when unfrozen.
        let mut legacy_cellbase_key = [0u8; 36];
        legacy_cellbase_key[..BLOCK_HASH_SIZE].copy_from_slice(hash);
        if let Some(old_cellbase) =
            source.get_pinned(legacy::COLUMN_BLOCK_BODY, &legacy_cellbase_key)?
        {
            if target
                .get_pinned(COLUMN_BLOCK_BODY, &block_body_key_bytes(number, hash, 0))?
                .as_deref()
                != Some(old_cellbase.as_ref())
            {
                return Err(internal_error(format!(
                    "migrated DB cellbase mismatch at height {number}"
                )));
            }

            let mut old_body_iter = source.iter(
                legacy::COLUMN_BLOCK_BODY,
                IteratorMode::From(hash, Direction::Forward),
            )?;
            let old_txs = old_body_iter
                .by_ref()
                .take_while(|(k, _)| k.starts_with(hash))
                .count();
            check_iter_status(&old_body_iter, legacy::COLUMN_BLOCK_BODY)?;

            let mut new_body_iter = target.iter(
                COLUMN_BLOCK_BODY,
                IteratorMode::From(&bk, Direction::Forward),
            )?;
            let new_txs = new_body_iter
                .by_ref()
                .take_while(|(k, _)| k.starts_with(&bk))
                .count();
            check_iter_status(&new_body_iter, COLUMN_BLOCK_BODY)?;

            if old_txs != new_txs {
                return Err(internal_error(format!(
                    "migrated DB block body tx count mismatch at height {number}: expected {old_txs}, actual {new_txs}"
                )));
            }
        }
    }

    let tip_key = block_key_bytes(plan.tip_number, &plan.tip_hash);
    if target.get_pinned(COLUMN_BLOCK_HEADER, &tip_key)?.is_none() {
        return Err(internal_error("migrated DB is missing tip header"));
    }
    validate_fork_hash_index(target, plan)
}

fn validate_ingest_counts(
    plan: &ValidationPlan,
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

    for (col, expected) in [
        (COLUMN_BLOCK_HEADER, plan.block_count),
        (COLUMN_HASH_INDEX, plan.block_count),
        (COLUMN_INDEX, plan.canonical_count),
    ] {
        let actual = expected_entries.get(col).copied().unwrap_or(0);
        if actual != expected {
            return Err(internal_error(format!(
                "migration generated {actual} entries for column {col}, expected {expected}"
            )));
        }
    }
    Ok(())
}

fn validate_fork_hash_index(target: &RocksDB, plan: &ValidationPlan) -> Result<(), Error> {
    for (number, hashes) in &plan.forks_by_height {
        for hash in hashes {
            let bk = block_key_bytes(*number, hash);
            if target.get_pinned(COLUMN_BLOCK_HEADER, &bk)?.is_none() {
                return Err(internal_error(format!(
                    "migrated DB is missing fork header at height {number}"
                )));
            }
            let index_value = target
                .get_pinned(COLUMN_HASH_INDEX, hash)?
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

fn get_required(source: &RocksDBSnapshot, col: Col, key: &[u8]) -> Result<Vec<u8>, Error> {
    source
        .get_pinned(col, key)?
        .map(|slice| slice.to_vec())
        .ok_or_else(|| {
            internal_error(format!(
                "required key with {} bytes missing from column {col}",
                key.len()
            ))
        })
}

fn hash_bytes_from_slice(slice: &[u8], label: &str) -> Result<BlockHashBytes, Error> {
    slice.try_into().map_err(|_| {
        internal_error(format!(
            "invalid {label}: expected {BLOCK_HASH_SIZE} bytes, actual {}",
            slice.len()
        ))
    })
}

fn header_number(value: &[u8]) -> Result<BlockNumber, Error> {
    let reader = packed::HeaderViewReader::from_slice(value)
        .map_err(|err| internal_error(format!("invalid block header value: {err}")))?;
    Ok(reader.data().raw().number().into())
}

fn block_number_from_index_key(key: &[u8]) -> Result<BlockNumber, Error> {
    let number = packed::Uint64::from_slice(key)
        .map_err(|err| internal_error(format!("invalid COLUMN_INDEX key: {err}")))?;
    Ok(number.as_reader().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_db_path_normalizes_trailing_dot_and_rejects_non_normal_leaf() {
        assert_eq!(
            clean_db_path(Path::new("/a/b/db/.")),
            PathBuf::from("/a/b/db")
        );
        assert_eq!(
            clean_db_path(Path::new("../../db/")),
            PathBuf::from("../../db")
        );
        assert!(SstRebuild::new(PathBuf::from(".")).is_err());
        assert!(SstRebuild::new(PathBuf::from("..")).is_err());
        assert!(SstRebuild::new(PathBuf::from("/")).is_err());
    }
}
