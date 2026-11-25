//! RocksDB wrapper base on OptimisticTransactionDB
use crate::snapshot::RocksDBSnapshot;
use crate::transaction::RocksDBTransaction;
use crate::write_batch::RocksDBWriteBatch;
use crate::{Result, internal_error};
use ckb_app_config::DBConfig;
use ckb_db_schema::{Col, legacy, v1};
use ckb_logger::info;
use rocksdb::ops::{
    CompactRangeCF, CreateCF, DropCF, Flush, GetColumnFamilys, GetPinned, GetPinnedCF,
    GetPropertyCF, IngestExternalFileCF, IterateCF, Open, OpenCF, Put, SetOptions, WriteOps,
};
use rocksdb::{
    BlockBasedIndexType, BlockBasedOptions, Cache, ColumnFamily, ColumnFamilyDescriptor, DB,
    DBPinnableSlice, Env, FlushOptions, FullOptions, IngestExternalFileOptions, IteratorMode,
    OptimisticTransactionDB, OptimisticTransactionOptions, Options, ReadOnlyDB as RawReadOnlyDB,
    SliceTransform, WriteBatch, WriteOptions, ffi,
};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

const PROPERTY_NUM_KEYS: &str = "rocksdb.estimate-num-keys";

/// RocksDB column-family schema.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Schema {
    /// Current CKB column-family schema.
    V1,
    /// Legacy numeric column-family schema used by databases before the v1 rebuild.
    Legacy,
}

impl Schema {
    fn column_families(self) -> &'static [Col] {
        match self {
            Schema::V1 => v1::COLUMN_FAMILIES,
            Schema::Legacy => legacy::COLUMN_FAMILIES,
        }
    }
}

/// RocksDB wrapper base on OptimisticTransactionDB
///
/// <https://github.com/facebook/rocksdb/wiki/Transactions#optimistictransactiondb>
#[derive(Clone)]
pub struct RocksDB {
    pub(crate) inner: Arc<OptimisticTransactionDB>,
}

const DEFAULT_CACHE_SIZE: usize = 256 << 20;
const DEFAULT_CACHE_ENTRY_CHARGE_SIZE: usize = 4096;
const DEFAULT_NUM_LEVELS: i32 = 7;
const MIGRATION_MAX_OPEN_FILES: i32 = 256;

const LEGACY_COLUMN_FAMILY_OPTION_MAPPINGS: &[(Col, Option<Col>, &str)] = &[
    (legacy::COLUMN_INDEX, Some(v1::COLUMN_INDEX), "rename"),
    (
        legacy::COLUMN_BLOCK_HEADER,
        Some(v1::COLUMN_BLOCK_HEADER),
        "rename",
    ),
    (
        legacy::COLUMN_BLOCK_BODY,
        Some(v1::COLUMN_BLOCK_BODY),
        "rename",
    ),
    (
        legacy::COLUMN_BLOCK_UNCLE,
        Some(v1::COLUMN_BLOCK_UNCLE),
        "rename",
    ),
    (legacy::COLUMN_META, Some(v1::COLUMN_META), "rename"),
    (
        legacy::COLUMN_TRANSACTION_INFO,
        Some(v1::COLUMN_TRANSACTION_INFO),
        "rename",
    ),
    (
        legacy::COLUMN_BLOCK_EXT,
        Some(v1::COLUMN_BLOCK_EXT),
        "rename",
    ),
    (
        legacy::COLUMN_BLOCK_PROPOSAL_IDS,
        Some(v1::COLUMN_BLOCK_PROPOSAL_IDS),
        "rename",
    ),
    (
        legacy::COLUMN_BLOCK_EPOCH,
        Some(v1::COLUMN_BLOCK_EPOCH),
        "rename",
    ),
    (legacy::COLUMN_EPOCH, Some(v1::COLUMN_EPOCH), "rename"),
    (legacy::COLUMN_CELL, Some(v1::COLUMN_CELL), "rename"),
    (legacy::COLUMN_UNCLES, Some(v1::COLUMN_UNCLES), "rename"),
    (
        legacy::COLUMN_CELL_DATA,
        Some(v1::COLUMN_CELL_DATA),
        "rename",
    ),
    (legacy::COLUMN_NUMBER_HASH, None, "removed"),
    (
        legacy::COLUMN_CELL_DATA_HASH,
        Some(v1::COLUMN_CELL_DATA_HASH),
        "rename",
    ),
    (
        legacy::COLUMN_BLOCK_EXTENSION,
        Some(v1::COLUMN_BLOCK_EXTENSION),
        "rename",
    ),
    (
        legacy::COLUMN_CHAIN_ROOT_MMR,
        Some(v1::COLUMN_CHAIN_ROOT_MMR),
        "rename",
    ),
    (
        legacy::COLUMN_BLOCK_FILTER,
        Some(v1::COLUMN_BLOCK_FILTER),
        "rename",
    ),
    (
        legacy::COLUMN_BLOCK_FILTER_HASH,
        Some(v1::COLUMN_BLOCK_FILTER_HASH),
        "rename",
    ),
];

fn legacy_column_family_option_hint(name: &str) -> Option<String> {
    let (_, new_name, note) = LEGACY_COLUMN_FAMILY_OPTION_MAPPINGS
        .iter()
        .find(|(old_name, _, _)| *old_name == name)?;
    Some(match new_name {
        Some(new_name) => format!("[CFOptions \"{name}\"] -> [CFOptions \"{new_name}\"]"),
        None => format!("[CFOptions \"{name}\"] has no direct current column family; {note}"),
    })
}

fn legacy_column_family_options_table() -> String {
    let mut table = String::from(
        "Legacy-to-current column-family options mapping:\n  old CF | current CF           | note\n  ------ | -------------------- | ----",
    );
    for (old_name, new_name, note) in LEGACY_COLUMN_FAMILY_OPTION_MAPPINGS {
        let old_name = format!("\"{old_name}\"");
        let new_name = new_name
            .map(|name| format!("\"{name}\""))
            .unwrap_or_else(|| "(removed)".to_string());
        table.push_str(&format!("\n  {old_name:<6} | {new_name:<20} | {note}"));
    }
    table.push_str(&format!(
        "\n  (new)  | \"{}\"         | new hash-to-block index column; configure separately if needed",
        v1::COLUMN_HASH_INDEX
    ));
    table
}

fn legacy_column_family_options_hint(unknown_cf_names: &[&str]) -> Option<String> {
    let hints: Vec<_> = unknown_cf_names
        .iter()
        .filter_map(|name| legacy_column_family_option_hint(name))
        .collect();
    if hints.is_empty() {
        None
    } else {
        Some(format!(
            "If you keep per-column-family tuning, rename the reported legacy sections as: {}.\n{}",
            hints.join("; "),
            legacy_column_family_options_table()
        ))
    }
}

/// Apply the canonical BlockBasedTable and prefix-extractor options for offline SST creation
/// and validation, with the per-CF block cache disabled.
pub fn apply_sst_table_options(col: &str, opts: &mut Options) {
    let mut block_opts = build_block_based_options(col, opts);
    block_opts.disable_cache();
    opts.set_block_based_table_factory(&block_opts);
}

fn disable_block_cache(opts: &mut Options) {
    let mut block_opts = BlockBasedOptions::default();
    block_opts.disable_cache();
    opts.set_block_based_table_factory(&block_opts);
}

fn build_block_based_options(col: &str, opts: &mut Options) -> BlockBasedOptions {
    let mut block_opts = BlockBasedOptions::default();
    block_opts.set_ribbon_filter(10.0);
    block_opts.set_index_type(BlockBasedIndexType::TwoLevelIndexSearch);
    block_opts.set_partition_filters(true);
    block_opts.set_metadata_block_size(4096);
    block_opts.set_pin_top_level_index_and_filter(true);

    if col == v1::COLUMN_BLOCK_BODY {
        // V1 block-body key: block_number (8) + block_hash (32) + tx_index (4).
        // Prefix is block_key (40 bytes) to group transactions by block.
        block_opts.set_whole_key_filtering(false);
        opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(40));
    } else if col == legacy::COLUMN_BLOCK_BODY {
        // Legacy block-body key: block_hash (32) + tx_index (4).
        block_opts.set_whole_key_filtering(false);
        opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(32));
    }

    block_opts
}

fn load_options_file(
    file: &Path,
    cache: Option<Cache>,
    cf_names: &[String],
) -> Result<(Options, Vec<ColumnFamilyDescriptor>)> {
    let mut full_opts = FullOptions::load_from_file_with_cache(file, cache, false)
        .map_err(|err| internal_error(format!("failed to load the options file: {err}")))?;
    let cf_names_str: Vec<&str> = cf_names.iter().map(|s| s.as_str()).collect();
    let loaded_cf_names: Vec<_> = full_opts
        .cf_descriptors
        .iter()
        .map(|cf| cf.name().to_string())
        .collect();
    let unknown_cf_names: Vec<_> = loaded_cf_names
        .iter()
        .map(String::as_str)
        .filter(|name| {
            *name != "default" && !cf_names.iter().any(|cf_name| cf_name.as_str() == *name)
        })
        .collect();
    full_opts
        .complete_column_families(&cf_names_str, false)
        .map_err(|err| {
            let unknown_cf_hint = if unknown_cf_names.is_empty() {
                "no unknown column family was detected before validation".to_string()
            } else {
                format!("unknown column families: {unknown_cf_names:?}")
            };
            let legacy_cf_hint = legacy_column_family_options_hint(&unknown_cf_names)
                .unwrap_or_else(|| {
                    "Check that every [CFOptions \"...\"] section uses a current column-family name"
                        .to_string()
                });
            internal_error(format!(
                "RocksDB options file {} is incompatible with the current CKB DB schema.\n\n\
Problem:\n  {unknown_cf_hint}\n\n\
Likely cause:\n  The options file was generated by an older CKB version with numeric column-family names, such as [CFOptions \"1\"].\n\n\
How to update column-family options:\n{legacy_cf_hint}\n\n\
How to fix:\n  Update the column-family names in the RocksDB options file, for example default.db-options, or remove legacy numeric [CFOptions \"...\"] sections you do not need.\n\n\
RocksDB error:\n  {err}",
                file.display(),
            ))
        })?;
    let FullOptions {
        db_opts,
        cf_descriptors,
    } = full_opts;
    Ok((db_opts, cf_descriptors))
}

#[derive(Default)]
struct ParsedOptionsFile {
    wal_dir: Option<PathBuf>,
    default_num_levels: Option<i32>,
    cf_num_levels: Vec<(String, i32)>,
}

enum OptionsSection {
    DBOptions,
    CFOptions(String),
    Other,
}

fn is_rocksdb_space(byte: u8) -> bool {
    // Match C-locale `isspace` used by RocksDB's `TrimAndRemoveComment` (never strip non-ASCII
    // Unicode whitespace such as U+00A0).
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn trim_rocksdb_space(mut slice: &[u8]) -> &[u8] {
    while let Some(&first) = slice.first() {
        if is_rocksdb_space(first) {
            slice = &slice[1..];
        } else {
            break;
        }
    }
    while let Some(&last) = slice.last() {
        if is_rocksdb_space(last) {
            slice = &slice[..slice.len() - 1];
        } else {
            break;
        }
    }
    slice
}

fn trim_and_remove_comment_bytes(line: &[u8]) -> &[u8] {
    for (i, &byte) in line.iter().enumerate() {
        if byte == b'#' && (i == 0 || line[i - 1] != b'\\') {
            return trim_rocksdb_space(&line[..i]);
        }
    }
    trim_rocksdb_space(line)
}

fn unescape_option_bytes(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut escaped = false;
    for &byte in raw {
        if escaped {
            out.push(match byte {
                b'r' => b'\r',
                b'n' => b'\n',
                other => other,
            });
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else {
            out.push(byte);
        }
    }
    out
}

fn path_from_option_bytes(bytes: Vec<u8>) -> PathBuf {
    #[cfg(unix)]
    {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(OsString::from_vec(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(&bytes).into_owned())
    }
}

fn parse_options_file(file: &Path) -> Result<ParsedOptionsFile> {
    let content = fs::read(file).map_err(|err| {
        internal_error(format!(
            "failed to read options file {}: {err}",
            file.display()
        ))
    })?;
    let mut parsed = ParsedOptionsFile::default();
    let mut section: Option<OptionsSection> = None;

    for raw_line in content.split(|&b| b == b'\n') {
        let line = trim_and_remove_comment_bytes(raw_line);
        if line.is_empty() {
            continue;
        }
        if line.starts_with(b"[") && line.ends_with(b"]") && line.len() >= 2 {
            let inner = &line[1..line.len() - 1];
            let first_quote = inner.iter().position(|&b| b == b'"');
            let last_quote = inner.iter().rposition(|&b| b == b'"');
            let (title, arg) = match (first_quote, last_quote) {
                (Some(start), Some(end)) if start < end => (
                    trim_rocksdb_space(&inner[..start]),
                    String::from_utf8_lossy(&unescape_option_bytes(&inner[start + 1..end]))
                        .into_owned(),
                ),
                _ => (trim_rocksdb_space(inner), String::new()),
            };
            section = Some(match title {
                b"DBOptions" => OptionsSection::DBOptions,
                b"CFOptions" => OptionsSection::CFOptions(arg),
                _ => OptionsSection::Other,
            });
            continue;
        }

        let Some(eq_pos) = line.iter().position(|&b| b == b'=') else {
            continue;
        };
        let key = trim_rocksdb_space(&line[..eq_pos]);
        let value = trim_and_remove_comment_bytes(&line[eq_pos + 1..]);
        // Match RocksDB's RocksDBOptionsParser (`opt_map.insert({name, value})`):
        // within a section, the first occurrence of a key wins and duplicates are ignored.
        match section.as_ref() {
            Some(OptionsSection::DBOptions) if key == b"wal_dir" && parsed.wal_dir.is_none() => {
                parsed.wal_dir = Some(path_from_option_bytes(unescape_option_bytes(value)));
            }
            Some(OptionsSection::CFOptions(cf_name)) if key == b"num_levels" => {
                if let Ok(value_str) = std::str::from_utf8(value)
                    && let Ok(levels) = value_str.parse::<i32>()
                {
                    if cf_name == "default" {
                        if parsed.default_num_levels.is_none() {
                            parsed.default_num_levels = Some(levels);
                        }
                    } else if !parsed.cf_num_levels.iter().any(|(name, _)| name == cf_name) {
                        parsed.cf_num_levels.push((cf_name.clone(), levels));
                    }
                }
            }
            _ => {}
        }
    }

    Ok(parsed)
}

fn rocksdb_wal_dir_matches(db_opts: &Options, expected_wal_dir: Option<&Path>) -> Result<bool> {
    const PROBE_DB_PATH: &str = "/__ckb_wal_dir_probe__";
    const PROBE_KEY: &[u8] = b"__ckb_wal_dir_probe_key__";
    const PROBE_VAL: &[u8] = b"1";

    let env = Env::mem_env()
        .map_err(|err| internal_error(format!("failed to create RocksDB mem env: {err}")))?;

    let mut writer_opts = Options::default();
    writer_opts.set_env(&env);
    writer_opts.create_if_missing(true);
    if let Some(wal_dir) = expected_wal_dir {
        writer_opts.set_wal_dir(wal_dir);
    }
    let writer = DB::open(&writer_opts, PROBE_DB_PATH).map_err(|err| {
        internal_error(format!("failed to create WAL probe DB in mem env: {err}"))
    })?;
    writer
        .put(PROBE_KEY, PROBE_VAL)
        .map_err(|err| internal_error(format!("failed to write WAL probe key: {err}")))?;

    // Probe RocksDB's own parsed `db_opts.wal_dir` inside an isolated in-memory Env without
    // touching the real filesystem. Because `PROBE_KEY` has not been flushed to an SST,
    // `RawReadOnlyDB` only reads it when RocksDB's parsed `wal_dir` resolves to the exact
    // directory holding the probe WAL.
    let mut probe_opts = db_opts.clone();
    probe_opts.set_env(&env);
    probe_opts.set_db_log_dir(PROBE_DB_PATH);
    probe_opts.create_if_missing(false);
    probe_opts.set_use_direct_reads(false);
    probe_opts.set_use_direct_io_for_flush_and_compaction(false);

    let matched = match RawReadOnlyDB::open(&probe_opts, PROBE_DB_PATH) {
        Ok(ro) => ro.get_pinned(PROBE_KEY).map_err(internal_error)?.as_deref() == Some(PROBE_VAL),
        Err(_) => false,
    };
    drop(writer);
    Ok(matched)
}

fn normalize_path_for_comparison(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(path)
    } else {
        path.to_path_buf()
    };
    let mut normalized = PathBuf::new();
    for component in abs.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => normalized.push(component.as_os_str()),
            },
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn reject_separate_wal_dir(
    db_path: &Path,
    options_file: &Path,
    db_opts: &Options,
    parsed: &ParsedOptionsFile,
) -> Result<()> {
    // 1. Ask RocksDB itself (in-memory) whether `db_opts.wal_dir` is empty.
    if rocksdb_wal_dir_matches(db_opts, None)? {
        return Ok(());
    }

    // 2. RocksDB resolved a non-empty `wal_dir`. Require it to equal `db_path` AND confirm
    //    byte-for-byte with RocksDB itself so any parsing discrepancy fails closed.
    let Some(wal_dir) = parsed
        .wal_dir
        .as_deref()
        .filter(|dir| !dir.as_os_str().is_empty())
    else {
        return Err(internal_error(format!(
            "RocksDB options file {} configures a separate wal_dir outside database path {}; \
             offline migration does not support a separate WAL directory. \
             Flush and close the database and consolidate WAL files into the database directory before migrating.",
            options_file.display(),
            db_path.display(),
        )));
    };
    if normalize_path_for_comparison(wal_dir) != normalize_path_for_comparison(db_path)
        || !rocksdb_wal_dir_matches(db_opts, Some(wal_dir))?
    {
        return Err(internal_error(format!(
            "RocksDB options file {} configures a separate wal_dir ({}) outside database path {}; \
             offline migration does not support a separate WAL directory. \
             Flush and close the database and consolidate WAL files into the database directory before migrating.",
            options_file.display(),
            wal_dir.display(),
            db_path.display(),
        )));
    }
    Ok(())
}

impl RocksDB {
    /// Inspect the column families recorded in a RocksDB directory's MANIFEST without opening or
    /// modifying the database. Returns `Ok(None)` if no RocksDB database exists at `path`.
    pub fn detect_schema<P: AsRef<Path>>(path: P) -> Result<Option<Schema>> {
        let path = path.as_ref();
        if !path.join("CURRENT").is_file() {
            return Ok(None);
        }

        let opts = Options::default();
        let cf_names = match DB::list_cf(&opts, path) {
            Ok(names) => names,
            Err(err) => {
                let err_str = err.as_ref();
                if err_str.starts_with("IO error: No such file or directory") {
                    return Ok(None);
                } else if err_str.starts_with("Corruption:") {
                    info!("DB corrupted: {err_str}.");
                    return Err(internal_error("DB corrupted"));
                } else {
                    return Err(internal_error(format!(
                        "failed to inspect database column families: {err}"
                    )));
                }
            }
        };

        let has_v1 = cf_names
            .iter()
            .any(|name| v1::COLUMN_FAMILIES.contains(&name.as_str()));
        let has_legacy = cf_names
            .iter()
            .any(|name| legacy::COLUMN_FAMILIES.contains(&name.as_str()));

        match (has_v1, has_legacy) {
            (true, false) => Ok(Some(Schema::V1)),
            (false, true) => Ok(Some(Schema::Legacy)),
            (true, true) => Err(internal_error(format!(
                "database {} mixes legacy and V1 column families: {cf_names:?}",
                path.display()
            ))),
            (false, false) => Err(internal_error(format!(
                "database {} has unrecognized column families: {cf_names:?}",
                path.display()
            ))),
        }
    }

    /// Validate that a RocksDB options file is compatible with `schema`.
    pub fn validate_options_file(file: &Path, schema: Schema) -> Result<()> {
        let cf_names = schema_column_family_names(schema);
        load_options_file(file, None, &cf_names).map(|_| ())
    }

    /// Validate that `options_file` parses cleanly and does not configure a separate `wal_dir`
    /// outside `db_path`.
    pub fn check_options_file_wal_dir(db_path: &Path, options_file: &Path) -> Result<()> {
        let full_opts = FullOptions::load_from_file_with_cache(options_file, None, false)
            .map_err(|err| internal_error(format!("failed to load the options file: {err}")))?;
        let parsed = parse_options_file(options_file)?;
        reject_separate_wal_dir(db_path, options_file, &full_opts.db_opts, &parsed)
    }

    /// Validate that `options_file` is compatible with running `--sst-rebuild` on `db_path`.
    ///
    /// Rejects malformed options files, separate `wal_dir` configurations outside `db_path`,
    /// and V1-compatible options files whose effective `num_levels` is smaller than the
    /// 7-level L6 database produced by SST rebuild.
    pub fn check_options_file_for_sst_rebuild(db_path: &Path, options_file: &Path) -> Result<()> {
        let full_opts = FullOptions::load_from_file_with_cache(options_file, None, false)
            .map_err(|err| internal_error(format!("failed to load the options file: {err}")))?;
        let parsed = parse_options_file(options_file)?;
        reject_separate_wal_dir(db_path, options_file, &full_opts.db_opts, &parsed)?;

        let v1_cf_names = schema_column_family_names(Schema::V1);
        if load_options_file(options_file, None, &v1_cf_names).is_ok() {
            let effective_num_levels = |cf_name: &str| -> i32 {
                parsed
                    .cf_num_levels
                    .iter()
                    .find(|(name, _)| name == cf_name)
                    .map(|(_, levels)| *levels)
                    .or(parsed.default_num_levels)
                    .unwrap_or(DEFAULT_NUM_LEVELS)
            };
            for cf_name in std::iter::once("default").chain(v1::COLUMN_FAMILIES.iter().copied()) {
                let num_levels = effective_num_levels(cf_name);
                if num_levels < DEFAULT_NUM_LEVELS {
                    return Err(internal_error(format!(
                        "RocksDB options file {} configures num_levels={num_levels} for column family \"{cf_name}\", \
                         which is incompatible with the {DEFAULT_NUM_LEVELS}-level L6 database built by --sst-rebuild; \
                         set num_levels >= {DEFAULT_NUM_LEVELS} before migrating.",
                        options_file.display(),
                    )));
                }
            }
        }

        Ok(())
    }

    /// Open a database with the given configuration and schema, returning an error on failure.
    pub fn open_with_check(config: &DBConfig, schema: Schema) -> Result<Self> {
        Self::open_with_column_family_names(config, schema_column_family_names(schema))
    }

    pub(crate) fn open_with_check_columns(config: &DBConfig, columns: u32) -> Result<Self> {
        Self::open_with_column_family_names(config, numeric_column_family_names(columns))
    }

    fn open_with_column_family_names(config: &DBConfig, cf_names: Vec<String>) -> Result<Self> {
        let mut cache = None;

        let (mut opts, mut cf_descriptors) = if let Some(ref file) = config.options_file {
            cache = match config.cache_size {
                Some(0) => None,
                Some(size) => Some(Cache::new_hyper_clock_cache(
                    size,
                    DEFAULT_CACHE_ENTRY_CHARGE_SIZE,
                )),
                None => Some(Cache::new_hyper_clock_cache(
                    DEFAULT_CACHE_SIZE,
                    DEFAULT_CACHE_ENTRY_CHARGE_SIZE,
                )),
            };

            load_options_file(file, cache.clone(), &cf_names)?
        } else {
            let opts = Options::default();
            let cf_descriptors: Vec<_> = cf_names
                .iter()
                .map(|c| ColumnFamilyDescriptor::new(c, Options::default()))
                .collect();
            (opts, cf_descriptors)
        };

        for cf in cf_descriptors.iter_mut() {
            let cf_name = cf.name().to_string();
            let mut block_opts = build_block_based_options(&cf_name, &mut cf.options);
            match cache {
                Some(ref cache) => {
                    block_opts.set_block_cache(cache);
                    block_opts.set_cache_index_and_filter_blocks(true);
                    block_opts.set_pin_l0_filter_and_index_blocks_in_cache(true);
                }
                None => block_opts.disable_cache(),
            }
            cf.options.set_block_based_table_factory(&block_opts);
        }

        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        opts.enable_statistics();

        let db = OptimisticTransactionDB::open_cf_descriptors(&opts, &config.path, cf_descriptors)
            .map_err(|err| internal_error(format!("failed to open database: {err}")))?;

        if !config.options.is_empty() {
            let rocksdb_options: Vec<(&str, &str)> = config
                .options
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            db.set_options(&rocksdb_options)
                .map_err(|_| internal_error("failed to set database option"))?;
        }

        Ok(RocksDB {
            inner: Arc::new(db),
        })
    }

    /// Open a database with the given configuration and schema.
    pub fn open(config: &DBConfig, schema: Schema) -> Self {
        Self::open_with_check(config, schema).unwrap_or_else(|err| panic!("{err}"))
    }

    /// Open a database with numeric column-family names `0..columns`.
    pub fn open_with_columns(config: &DBConfig, columns: u32) -> Self {
        Self::open_with_check_columns(config, columns).unwrap_or_else(|err| panic!("{err}"))
    }

    /// Open a database in the given directory with the default configuration and schema.
    pub fn open_in<P: AsRef<Path>>(path: P, schema: Schema) -> Self {
        let config = DBConfig {
            path: path.as_ref().to_path_buf(),
            ..Default::default()
        };
        Self::open_with_check(&config, schema).unwrap_or_else(|err| panic!("{err}"))
    }

    /// Set appropriate parameters for bulk loading.
    pub fn prepare_for_bulk_load_open<P: AsRef<Path>>(
        path: P,
        schema: Schema,
    ) -> Result<Option<Self>> {
        Self::prepare_for_bulk_load_open_with_options(path, schema, None)
    }

    /// Set appropriate parameters for bulk loading, rejecting separate `wal_dir` configurations
    /// in `options_file` before any writable open.
    pub fn prepare_for_bulk_load_open_with_options<P: AsRef<Path>>(
        path: P,
        schema: Schema,
        options_file: Option<&Path>,
    ) -> Result<Option<Self>> {
        let path = path.as_ref();
        if !path.join("CURRENT").is_file() {
            return Ok(None);
        }
        if let Some(file) = options_file {
            Self::check_options_file_wal_dir(path, file)?;
        }
        Self::bulk_load_open(path, schema, false)
    }

    /// Create a database with appropriate parameters for bulk loading.
    pub fn create_for_bulk_load_open<P: AsRef<Path>>(path: P, schema: Schema) -> Result<Self> {
        let path = path.as_ref();
        Self::bulk_load_open(path, schema, true)?.ok_or_else(|| {
            internal_error(format!(
                "failed to create bulk-load database {}",
                path.display()
            ))
        })
    }

    /// Open an existing database exclusively for offline migration without creating missing
    /// files or column families and with background compactions disabled.
    pub fn open_existing_for_migration<P: AsRef<Path>>(
        path: P,
        schema: Schema,
    ) -> Result<Option<Self>> {
        Self::open_existing_for_migration_with_options(path, schema, None)
    }

    /// Open an existing database exclusively for offline migration using DB/CF options from
    /// `options_file` when provided, rejecting separate `wal_dir` configurations before opening.
    pub fn open_existing_for_migration_with_options<P: AsRef<Path>>(
        path: P,
        schema: Schema,
        options_file: Option<&Path>,
    ) -> Result<Option<Self>> {
        let path = path.as_ref();
        if !path.join("CURRENT").is_file() {
            return Ok(None);
        }

        let (mut opts, cf_descriptors) = if let Some(file) = options_file {
            let full_opts = FullOptions::load_from_file_with_cache(file, None, false)
                .map_err(|err| internal_error(format!("failed to load the options file: {err}")))?;
            let parsed = parse_options_file(file)?;
            reject_separate_wal_dir(path, file, &full_opts.db_opts, &parsed)?;

            let default_cf_opts = full_opts
                .cf_descriptors
                .iter()
                .find(|cf| cf.name() == "default")
                .map(|cf| cf.options.clone())
                .unwrap_or_default();
            let build_cf = |name: String, mut cf_opts: Options| {
                cf_opts.set_disable_auto_compactions(true);
                disable_block_cache(&mut cf_opts);
                ColumnFamilyDescriptor::new(name, cf_opts)
            };
            let mut cf_descriptors = Vec::with_capacity(schema.column_families().len() + 1);
            cf_descriptors.push(build_cf("default".to_string(), default_cf_opts.clone()));
            for name in schema_column_family_names(schema) {
                let cf_opts = full_opts
                    .cf_descriptors
                    .iter()
                    .find(|cf| cf.name() == name)
                    .map(|cf| cf.options.clone())
                    .unwrap_or_else(|| default_cf_opts.clone());
                cf_descriptors.push(build_cf(name, cf_opts));
            }
            (full_opts.db_opts, cf_descriptors)
        } else {
            let cf_descriptors = schema_column_family_names(schema)
                .into_iter()
                .map(|name| {
                    let mut cf_opts = Options::default();
                    cf_opts.set_disable_auto_compactions(true);
                    disable_block_cache(&mut cf_opts);
                    ColumnFamilyDescriptor::new(name, cf_opts)
                })
                .collect();
            (Options::default(), cf_descriptors)
        };

        opts.create_if_missing(false);
        opts.create_missing_column_families(false);
        opts.set_disable_auto_compactions(true);
        opts.set_max_open_files(MIGRATION_MAX_OPEN_FILES);
        disable_block_cache(&mut opts);

        Self::open_cf_descriptors_opt(&opts, path, cf_descriptors)
    }

    /// Open an existing database with production table options (filters, two-level index,
    /// prefix extractor) for offline validation, with bounded open files and no background
    /// compactions or per-CF block caches.
    pub fn open_existing_for_validation<P: AsRef<Path>>(path: P, schema: Schema) -> Result<Self> {
        Self::open_existing_for_validation_with_options(path, schema, None)
    }

    /// Open an existing database for offline validation, optionally using the effective V1
    /// column-family options from `options_file` so option incompatibilities (such as
    /// `num_levels < 7`) fail before cutover.
    pub fn open_existing_for_validation_with_options<P: AsRef<Path>>(
        path: P,
        schema: Schema,
        options_file: Option<&Path>,
    ) -> Result<Self> {
        let path = path.as_ref();
        if !path.join("CURRENT").is_file() {
            return Err(internal_error(format!(
                "database path {} does not exist",
                path.display()
            )));
        }

        let cf_names = schema_column_family_names(schema);
        let (mut opts, mut cf_descriptors) = if let Some(file) = options_file {
            let (mut db_opts, cf_descriptors) = load_options_file(file, None, &cf_names)?;
            // Never inherit a WAL path pointing at another directory when validating a staged DB.
            db_opts.set_wal_dir("");
            (db_opts, cf_descriptors)
        } else {
            let cf_descriptors = cf_names
                .into_iter()
                .map(|name| ColumnFamilyDescriptor::new(name, Options::default()))
                .collect();
            (Options::default(), cf_descriptors)
        };

        opts.create_if_missing(false);
        opts.create_missing_column_families(false);
        opts.set_disable_auto_compactions(true);
        opts.set_max_open_files(MIGRATION_MAX_OPEN_FILES);
        disable_block_cache(&mut opts);

        for cf in &mut cf_descriptors {
            let cf_name = cf.name().to_string();
            cf.options.set_disable_auto_compactions(true);
            apply_sst_table_options(&cf_name, &mut cf.options);
        }

        Self::open_cf_descriptors_opt(&opts, path, cf_descriptors)?.ok_or_else(|| {
            internal_error(format!("database path {} does not exist", path.display()))
        })
    }

    fn bulk_load_open<P: AsRef<Path>>(
        path: P,
        schema: Schema,
        create_if_missing: bool,
    ) -> Result<Option<Self>> {
        let path = path.as_ref();
        if !create_if_missing && !path.join("CURRENT").is_file() {
            return Ok(None);
        }

        let configure = |o: &mut Options| {
            o.set_prepare_for_bulk_load();
            // Restore standard 7 levels + dynamic level bytes after PrepareForBulkLoad (which
            // forces num_levels=2): existing L0..L6 DBs open cleanly and ingested SSTs land at L6.
            o.set_num_levels(DEFAULT_NUM_LEVELS);
            o.set_level_compaction_dynamic_level_bytes(true);
            disable_block_cache(o);
        };

        let mut opts = Options::default();
        opts.create_if_missing(create_if_missing);
        opts.create_missing_column_families(true);
        configure(&mut opts);
        opts.set_max_open_files(MIGRATION_MAX_OPEN_FILES);

        let cf_descriptors = schema_column_family_names(schema)
            .into_iter()
            .map(|name| {
                let mut cf_opts = Options::default();
                configure(&mut cf_opts);
                ColumnFamilyDescriptor::new(name, cf_opts)
            })
            .collect();

        Self::open_cf_descriptors_opt(&opts, path, cf_descriptors)
    }

    fn open_cf_descriptors_opt(
        opts: &Options,
        path: &Path,
        cf_descriptors: Vec<ColumnFamilyDescriptor>,
    ) -> Result<Option<Self>> {
        OptimisticTransactionDB::open_cf_descriptors(opts, path, cf_descriptors).map_or_else(
            |err| {
                let err_str = err.as_ref();
                if err_str.starts_with("Invalid argument:")
                    && err_str.ends_with("does not exist (create_if_missing is false)")
                {
                    Ok(None)
                } else if err_str.starts_with("Corruption:") {
                    info!("DB corrupted: {err_str}.");
                    Err(internal_error(err_str))
                } else {
                    Err(internal_error(format!(
                        "failed to open the database: {err}"
                    )))
                }
            },
            |db| {
                Ok(Some(RocksDB {
                    inner: Arc::new(db),
                }))
            },
        )
    }

    /// Return the value associated with a key using RocksDB's PinnableSlice from the given column
    /// so as to avoid unnecessary memory copy.
    pub fn get_pinned(&self, col: Col, key: &[u8]) -> Result<Option<DBPinnableSlice<'_>>> {
        let cf = cf_handle(&self.inner, col)?;
        self.inner.get_pinned_cf(cf, key).map_err(internal_error)
    }

    /// Return the value associated with a key using RocksDB's PinnableSlice from the default column
    /// so as to avoid unnecessary memory copy.
    pub fn get_pinned_default(&self, key: &[u8]) -> Result<Option<DBPinnableSlice<'_>>> {
        self.inner.get_pinned(key).map_err(internal_error)
    }

    /// Insert a value into the database under the given key.
    pub fn put_default<K, V>(&self, key: K, value: V) -> Result<()>
    where
        K: AsRef<[u8]>,
        V: AsRef<[u8]>,
    {
        self.inner.put(key, value).map_err(internal_error)
    }

    /// Insert a value into the default column family with `WriteOptions::set_sync(true)`.
    pub fn put_default_sync<K, V>(&self, key: K, value: V) -> Result<()>
    where
        K: AsRef<[u8]>,
        V: AsRef<[u8]>,
    {
        let mut wo = WriteOptions::new();
        wo.set_sync(true);
        self.inner.put_opt(key, value, &wo).map_err(internal_error)
    }

    /// Flush the default column family memtable to disk and wait for completion.
    pub fn flush_default(&self) -> Result<()> {
        let mut flush_opts = FlushOptions::default();
        flush_opts.set_wait(true);
        self.inner.flush_opt(&flush_opts).map_err(internal_error)
    }

    /// Traverse database column with the given callback function.
    pub fn full_traverse<F>(&self, col: Col, callback: &mut F) -> Result<()>
    where
        F: FnMut(&[u8], &[u8]) -> Result<()>,
    {
        let cf = cf_handle(&self.inner, col)?;
        let iter = self
            .inner
            .full_iterator_cf(cf, IteratorMode::Start)
            .map_err(internal_error)?;
        for (key, val) in iter {
            callback(&key, &val)?;
        }
        Ok(())
    }

    /// Traverse database column with the given callback function.
    pub fn traverse<F>(
        &self,
        col: Col,
        callback: &mut F,
        mode: IteratorMode,
        limit: usize,
    ) -> Result<(usize, Vec<u8>)>
    where
        F: FnMut(&[u8], &[u8]) -> Result<()>,
    {
        let mut count: usize = 0;
        let mut next_key: Vec<u8> = vec![];
        let cf = cf_handle(&self.inner, col)?;
        let iter = self
            .inner
            .full_iterator_cf(cf, mode)
            .map_err(internal_error)?;
        for (key, val) in iter {
            if count > limit {
                next_key = key.to_vec();
                break;
            }

            callback(&key, &val)?;
            count += 1;
        }
        Ok((count, next_key))
    }

    /// Set a snapshot at start of transaction by setting set_snapshot=true
    pub fn transaction(&self) -> RocksDBTransaction {
        let write_options = WriteOptions::default();
        let mut transaction_options = OptimisticTransactionOptions::new();
        transaction_options.set_snapshot(true);

        RocksDBTransaction {
            db: Arc::clone(&self.inner),
            inner: self.inner.transaction(&write_options, &transaction_options),
        }
    }

    /// Construct `RocksDBWriteBatch` with default option.
    pub fn new_write_batch(&self) -> RocksDBWriteBatch {
        RocksDBWriteBatch {
            db: Arc::clone(&self.inner),
            inner: WriteBatch::default(),
        }
    }

    /// Write batch into transaction db.
    pub fn write(&self, batch: &RocksDBWriteBatch) -> Result<()> {
        self.inner.write(&batch.inner).map_err(internal_error)
    }

    /// WriteOptions set_sync true
    /// If true, the write will be flushed from the operating system
    /// buffer cache (by calling WritableFile::Sync()) before the write
    /// is considered complete.  If this flag is true, writes will be
    /// slower.
    ///
    /// If this flag is false, and the machine crashes, some recent
    /// writes may be lost.  Note that if it is just the process that
    /// crashes (i.e., the machine does not reboot), no writes will be
    /// lost even if sync==false.
    ///
    /// In other words, a DB write with sync==false has similar
    /// crash semantics as the "write()" system call.  A DB write
    /// with sync==true has similar crash semantics to a "write()"
    /// system call followed by "fdatasync()".
    ///
    /// Default: false
    pub fn write_sync(&self, batch: &RocksDBWriteBatch) -> Result<()> {
        let mut wo = WriteOptions::new();
        wo.set_sync(true);
        self.inner
            .write_opt(&batch.inner, &wo)
            .map_err(internal_error)
    }

    /// The begin and end arguments define the key range to be compacted.
    /// The behavior varies depending on the compaction style being used by the db.
    /// In case of universal and FIFO compaction styles, the begin and end arguments are ignored and all files are compacted.
    /// Also, files in each level are compacted and left in the same level.
    /// For leveled compaction style, all files containing keys in the given range are compacted to the last level containing files.
    /// If either begin or end are NULL, it is taken to mean the key before all keys in the db or the key after all keys respectively.
    ///
    /// If more than one thread calls manual compaction,
    /// only one will actually schedule it while the other threads will simply wait for
    /// the scheduled manual compaction to complete.
    ///
    /// CompactRange waits while compaction is performed on the background threads and thus is a blocking call.
    pub fn compact_range(&self, col: Col, start: Option<&[u8]>, end: Option<&[u8]>) -> Result<()> {
        let cf = cf_handle(&self.inner, col)?;
        self.inner.compact_range_cf(cf, start, end);
        Ok(())
    }

    /// Ingest external SST files into the given column family.
    ///
    /// The caller is responsible for creating SSTs with keys sorted according to RocksDB's
    /// comparator and for ensuring the ingested files do not contain conflicting ranges.
    pub fn ingest_external_files<P: AsRef<Path>>(&self, col: Col, paths: Vec<P>) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }

        let cf = cf_handle(&self.inner, col)?;
        let mut opts = IngestExternalFileOptions::default();
        opts.set_move_files(true);
        opts.set_snapshot_consistency(false);
        self.inner
            .ingest_external_file_opts(cf, paths, &opts)
            .map_err(internal_error)
    }

    /// Return `RocksDBSnapshot`.
    pub fn get_snapshot(&self) -> RocksDBSnapshot {
        unsafe {
            let snapshot = ffi::rocksdb_create_snapshot(self.inner.base_db_ptr());
            RocksDBSnapshot::new(&self.inner, snapshot)
        }
    }

    /// Return rocksdb `OptimisticTransactionDB`.
    pub fn inner(&self) -> Arc<OptimisticTransactionDB> {
        Arc::clone(&self.inner)
    }

    /// Create a new column family for the database.
    pub fn create_cf(&mut self, col: Col) -> Result<()> {
        let inner = Arc::get_mut(&mut self.inner)
            .ok_or_else(|| internal_error("create_cf get_mut failed"))?;
        let opts = Options::default();
        inner.create_cf(col, &opts).map_err(internal_error)
    }

    /// Delete column family.
    pub fn drop_cf(&mut self, col: Col) -> Result<()> {
        let inner = Arc::get_mut(&mut self.inner)
            .ok_or_else(|| internal_error("drop_cf get_mut failed"))?;
        inner.drop_cf(col).map_err(internal_error)
    }

    /// "rocksdb.estimate-num-keys" - returns estimated number of total keys in
    /// the active and unflushed immutable memtables and storage.
    pub fn estimate_num_keys_cf(&self, col: Col) -> Result<Option<u64>> {
        let cf = cf_handle(&self.inner, col)?;
        self.inner
            .property_int_value_cf(cf, PROPERTY_NUM_KEYS)
            .map_err(internal_error)
    }
}

fn schema_column_family_names(schema: Schema) -> Vec<String> {
    schema
        .column_families()
        .iter()
        .map(|col| col.to_string())
        .collect()
}

fn numeric_column_family_names(columns: u32) -> Vec<String> {
    (0..columns).map(|c| c.to_string()).collect()
}

#[inline]
pub(crate) fn cf_handle(db: &OptimisticTransactionDB, col: Col) -> Result<&ColumnFamily> {
    db.cf_handle(col)
        .ok_or_else(|| internal_error(format!("column {col} not found")))
}
