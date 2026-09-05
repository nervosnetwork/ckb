//! RocksDB wrapper base on OptimisticTransactionDB
use crate::snapshot::RocksDBSnapshot;
use crate::transaction::RocksDBTransaction;
use crate::write_batch::RocksDBWriteBatch;
use crate::{Result, internal_error};
use ckb_app_config::DBConfig;
use ckb_db_schema::{Col, legacy, v1};
use ckb_logger::info;
use rocksdb::ops::{
    CompactRangeCF, CreateCF, DropCF, GetColumnFamilys, GetPinned, GetPinnedCF, GetPropertyCF,
    IngestExternalFileCF, IterateCF, OpenCF, Put, SetOptions, WriteOps,
};
use rocksdb::{
    BlockBasedIndexType, BlockBasedOptions, Cache, ColumnFamily, ColumnFamilyDescriptor,
    DBPinnableSlice, FullOptions, IngestExternalFileOptions, IteratorMode, OptimisticTransactionDB,
    OptimisticTransactionOptions, Options, SliceTransform, WriteBatch, WriteOptions, ffi,
};
use std::path::Path;
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

impl RocksDB {
    pub(crate) fn open_with_check(config: &DBConfig, schema: Schema) -> Result<Self> {
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

            let mut full_opts = FullOptions::load_from_file_with_cache(file, cache.clone(), false)
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
                    let legacy_cf_hint =
                        legacy_column_family_options_hint(&unknown_cf_names).unwrap_or_else(
                            || {
                                "Check that every [CFOptions \"...\"] section uses a current column-family name"
                                    .to_string()
                            },
                        );
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
            (db_opts, cf_descriptors)
        } else {
            let opts = Options::default();
            let cf_descriptors: Vec<_> = cf_names
                .iter()
                .map(|c| ColumnFamilyDescriptor::new(c, Options::default()))
                .collect();
            (opts, cf_descriptors)
        };

        for cf in cf_descriptors.iter_mut() {
            let mut block_opts = BlockBasedOptions::default();
            block_opts.set_ribbon_filter(10.0);
            block_opts.set_index_type(BlockBasedIndexType::TwoLevelIndexSearch);
            block_opts.set_partition_filters(true);
            block_opts.set_metadata_block_size(4096);
            block_opts.set_pin_top_level_index_and_filter(true);
            match cache {
                Some(ref cache) => {
                    block_opts.set_block_cache(cache);
                    block_opts.set_cache_index_and_filter_blocks(true);
                    block_opts.set_pin_l0_filter_and_index_blocks_in_cache(true);
                }
                None => block_opts.disable_cache(),
            }
            if cf.name() == v1::COLUMN_BLOCK_BODY {
                // V1 block-body key: block_number (8) + block_hash (32) + tx_index (4).
                // Prefix is block_key (40 bytes) to group transactions by block.
                block_opts.set_whole_key_filtering(false);
                cf.options
                    .set_prefix_extractor(SliceTransform::create_fixed_prefix(40));
            } else if cf.name() == legacy::COLUMN_BLOCK_BODY {
                // Legacy block-body key: block_hash (32) + tx_index (4).
                block_opts.set_whole_key_filtering(false);
                cf.options
                    .set_prefix_extractor(SliceTransform::create_fixed_prefix(32));
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

    fn bulk_load_open<P: AsRef<Path>>(
        path: P,
        schema: Schema,
        create_if_missing: bool,
    ) -> Result<Option<Self>> {
        let path = path.as_ref();
        if !create_if_missing && !path.exists() {
            return Ok(None);
        }

        let mut opts = Options::default();

        opts.create_if_missing(create_if_missing);
        opts.create_missing_column_families(true);
        opts.set_prepare_for_bulk_load();

        let cfnames = schema_column_family_names(schema);
        let cf_options: Vec<&str> = cfnames.iter().map(|n| n as &str).collect();

        OptimisticTransactionDB::open_cf(&opts, path, cf_options).map_or_else(
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
