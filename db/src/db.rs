//! RocksDB wrapper base on OptimisticTransactionDB
use crate::generation::{
    GENERATION_KEY, Generation, PAYLOAD_COLUMNS, Routing, WriterGate, checkpoint_retirement,
    logical_name, physical_name,
};
use crate::iter::DBIterator;
use crate::snapshot::RocksDBSnapshot;
use crate::transaction::RocksDBTransaction;
use crate::write_batch::RocksDBWriteBatch;
use crate::{DBPinnableSlice, Result, internal_error};
use ckb_app_config::DBConfig;
use ckb_db_schema::{
    COLUMN_BLOCK_ARCHIVE, COLUMN_BLOCK_BODY, COLUMN_META, Col, META_ARCHIVE_NEXT_RECORD,
};
use ckb_logger::info;
use rocksdb::ops::{
    CompactRangeCF, GetColumnFamilys, GetPinned, GetPinnedCF, GetPropertyCF, OpenCF, Put,
    SetOptions, WriteOps,
};
use rocksdb::{
    BlockBasedIndexType, BlockBasedOptions, Cache, ColumnFamilyDescriptor, FullOptions,
    IteratorMode, OptimisticTransactionDB, OptimisticTransactionOptions, Options, ReadOptions,
    SliceTransform, WriteBatch, WriteOptions, ffi,
};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

const PROPERTY_NUM_KEYS: &str = "rocksdb.estimate-num-keys";

/// RocksDB wrapper base on OptimisticTransactionDB
///
/// <https://github.com/facebook/rocksdb/wiki/Transactions#optimistictransactiondb>
#[derive(Clone)]
pub struct RocksDB {
    pub(crate) inner: Arc<OptimisticTransactionDB>,
    pub(crate) routing: Arc<Routing>,
}

const DEFAULT_CACHE_SIZE: usize = 256 << 20;
const DEFAULT_CACHE_ENTRY_CHARGE_SIZE: usize = 4096;

impl RocksDB {
    pub(crate) fn open_with_check(config: &DBConfig, columns: u32) -> Result<Self> {
        let cf_names: Vec<_> = (0..columns).map(|c| c.to_string()).collect();
        let mut cache = None;

        let (mut opts, mut cf_descriptors) = if let Some(ref file) = config.options_file {
            cache = match config.cache_size.unwrap_or(DEFAULT_CACHE_SIZE) {
                0 => None,
                size => Some(Cache::new_hyper_clock_cache(
                    size,
                    DEFAULT_CACHE_ENTRY_CHARGE_SIZE,
                )),
            };

            let mut full_opts = FullOptions::load_from_file_with_cache(file, cache.clone(), false)
                .map_err(|err| internal_error(format!("failed to load the options file: {err}")))?;
            let cf_names_str: Vec<&str> = cf_names.iter().map(|s| s.as_str()).collect();
            full_opts
                .complete_column_families(&cf_names_str, false)
                .map_err(|err| {
                    internal_error(format!("failed to check all column families: {err}"))
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
            if cf.name() == COLUMN_BLOCK_BODY {
                block_opts.set_whole_key_filtering(false);
                cf.options
                    .set_prefix_extractor(SliceTransform::create_fixed_prefix(32));
            }
            cf.options.set_block_based_table_factory(&block_opts);
        }

        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        opts.enable_statistics();

        let payload_options = cf_descriptors
            .iter()
            .filter_map(|cf| logical_name(cf.name()).map(|col| (col, cf.options.clone())))
            .collect();
        let archive_options = cf_descriptors
            .iter()
            .find(|cf| cf.name() == COLUMN_BLOCK_ARCHIVE)
            .map(|cf| cf.options.clone());
        // Open exactly the existing physical families. A missing published family
        // must be reported, never recreated as an empty replacement.
        if config
            .path
            .join("CURRENT")
            .try_exists()
            .map_err(internal_error)?
        {
            let existing = rocksdb::DB::list_cf(&opts, &config.path).map_err(internal_error)?;
            let templates: BTreeMap<_, _> = cf_descriptors
                .into_iter()
                .map(|cf| (cf.name().to_owned(), cf.options))
                .collect();
            cf_descriptors = existing
                .into_iter()
                .map(|name| {
                    let logical = logical_name(&name).unwrap_or(&name);
                    let options = templates.get(logical).cloned().unwrap_or_default();
                    ColumnFamilyDescriptor::new(name, options)
                })
                .collect();
        }
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

        Self::from_native(
            db,
            payload_options,
            archive_options,
            config.options_file.is_none(),
        )
    }

    fn from_native(
        db: OptimisticTransactionDB,
        options: BTreeMap<Col, Options>,
        archive_options: Option<Options>,
        batch_default_payload_cfs: bool,
    ) -> Result<Self> {
        let id = db
            .get_pinned(GENERATION_KEY)
            .map_err(internal_error)?
            .map(|value| {
                let bytes: [u8; 8] = value
                    .as_ref()
                    .try_into()
                    .map_err(|_| internal_error("invalid freezer CF generation marker"))?;
                Ok::<_, ckb_error::Error>(u64::from_le_bytes(bytes))
            })
            .transpose()?
            .unwrap_or(0);
        let missing_archive = archive_options
            .as_ref()
            .filter(|_| db.cf_handle(COLUMN_BLOCK_ARCHIVE).is_none());
        if missing_archive.is_some() {
            let meta = db
                .cf_handle(COLUMN_META)
                .ok_or_else(|| internal_error("missing metadata column"))?;
            if id != 0
                || db
                    .get_pinned_cf(meta, META_ARCHIVE_NEXT_RECORD)
                    .map_err(internal_error)?
                    .is_some()
            {
                return Err(internal_error(
                    "database is missing its published archive index column",
                ));
            }
        }
        // Validate the whole generation before transferring or retiring any handle.
        for col in options.keys() {
            if db.cf_handle(&physical_name(id, col)).is_none() {
                return Err(internal_error(format!(
                    "published freezer generation {id} is missing column {col}"
                )));
            }
        }
        let (inner, mut owned_columns) = db.into_shared_columns();
        if let Some(options) = missing_archive {
            // A node that has never archived needs only a new empty index CF.
            // Published archive layouts were rejected above if this CF was lost.
            let column = inner
                .create_owned_cf(COLUMN_BLOCK_ARCHIVE, options)
                .map_err(internal_error)?;
            owned_columns.insert(COLUMN_BLOCK_ARCHIVE.to_owned(), column);
        }
        let mut columns = BTreeMap::new();
        let mut retired = false;
        for (name, owned) in owned_columns {
            let Some(col) = logical_name(&name) else {
                if name.starts_with("freezer.") {
                    return Err(internal_error(format!(
                        "invalid freezer column family name {name}"
                    )));
                }
                columns.insert(name, owned);
                continue;
            };
            if name == physical_name(id, col) {
                columns.insert(col.to_owned(), owned);
            } else {
                // An unpublished copy or a partially retired generation from an
                // interrupted collection has no readers after reopening.
                fail::fail_point!("freezer-gc-recovery-before-drop", |_| Err(internal_error(
                    "injected recovery failure before CF drop"
                )));
                owned.drop_from_database().map_err(internal_error)?;
                retired = true;
                fail::fail_point!("freezer-gc-recovery-after-drop", |_| Err(internal_error(
                    "injected recovery failure after CF drop"
                )));
            }
        }
        if retired {
            checkpoint_retirement(&inner)?;
        }
        Ok(Self {
            inner,
            routing: Arc::new(Routing {
                current: RwLock::new(Arc::new(Generation { id, columns })),
                options,
                batch_default_payload_cfs,
                gate: Arc::new(WriterGate::default()),
                collector: Mutex::new(()),
                retired: Mutex::new(Vec::new()),
            }),
        })
    }

    /// Open a database with the given configuration and columns count.
    pub fn open(config: &DBConfig, columns: u32) -> Self {
        Self::open_with_check(config, columns).unwrap_or_else(|err| panic!("{err}"))
    }

    /// Open a database in the given directory with the default configuration and columns count.
    pub fn open_in<P: AsRef<Path>>(path: P, columns: u32) -> Self {
        let config = DBConfig {
            path: path.as_ref().to_path_buf(),
            ..Default::default()
        };
        Self::open_with_check(&config, columns).unwrap_or_else(|err| panic!("{err}"))
    }

    /// Set appropriate parameters for bulk loading.
    pub fn prepare_for_bulk_load_open<P: AsRef<Path>>(
        path: P,
        columns: u32,
    ) -> Result<Option<Self>> {
        let mut opts = Options::default();

        opts.create_missing_column_families(true);
        opts.set_prepare_for_bulk_load();

        let path = path.as_ref();
        let cfnames: Vec<_> = if path.join("CURRENT").try_exists().map_err(internal_error)? {
            rocksdb::DB::list_cf(&opts, path).map_err(internal_error)?
        } else {
            (0..columns).map(|c| c.to_string()).collect()
        };
        let cf_options: Vec<&str> = cfnames.iter().map(|n| n.as_str()).collect();

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
                // Older databases have a contiguous prefix of numbered CFs.
                // Extend that prefix only before the first Freezer publication;
                // reopening a published layout must never recreate a lost CF.
                let count = cfnames
                    .iter()
                    .filter(|name| name.parse::<u32>().is_ok())
                    .count() as u32;
                let legacy = count < columns
                    && !cfnames.iter().any(|name| name.starts_with("freezer."))
                    && (0..count).all(|col| cfnames.contains(&col.to_string()));
                let db = if legacy
                    && db
                        .get_pinned(GENERATION_KEY)
                        .map_err(internal_error)?
                        .is_none()
                {
                    drop(db);
                    let names: Vec<_> = cfnames
                        .into_iter()
                        .chain((count..columns).map(|col| col.to_string()))
                        .collect();
                    OptimisticTransactionDB::open_cf(&opts, path, names).map_err(internal_error)?
                } else {
                    db
                };
                let options = PAYLOAD_COLUMNS
                    .into_iter()
                    .filter(|col| col.parse::<u32>().expect("numeric payload column") < columns)
                    .map(|col| (col, Options::default()))
                    .collect();
                let archive_options = (COLUMN_BLOCK_ARCHIVE
                    .parse::<u32>()
                    .expect("numeric archive column")
                    < columns)
                    .then(Options::default);
                Self::from_native(db, options, archive_options, false).map(Some)
            },
        )
    }

    /// Report whether an uncertain native publication requires reopening.
    pub fn check_writable(&self) -> Result<()> {
        self.routing.gate.check()
    }

    /// Retired physical generations still held by readers, iterators or values.
    pub fn pinned_retired_generations(&self) -> usize {
        self.collection_status().pinned_retired_generations
    }

    /// Sample collection resource occupancy without traversing stored records.
    pub fn collection_status(&self) -> crate::CollectionStatus {
        let (active_writers, writers_paused, dirty_bytes) = self.routing.gate.status();
        let mut retired = self
            .routing
            .retired
            .lock()
            .expect("retired generations poisoned");
        retired.retain(|(_, columns)| columns.iter().any(|column| column.strong_count() != 0));
        crate::CollectionStatus {
            active_writers,
            writers_paused,
            dirty_bytes,
            pinned_retired_generations: retired.len(),
            oldest_retired_age: retired
                .first()
                .map_or(std::time::Duration::ZERO, |(time, _)| time.elapsed()),
        }
    }

    /// Return the value associated with a key using RocksDB's PinnableSlice from the given column
    /// so as to avoid unnecessary memory copy.
    pub fn get_pinned(&self, col: Col, key: &[u8]) -> Result<Option<DBPinnableSlice<'_>>> {
        self.generation()
            .get_pinned(col, key, &ReadOptions::default())
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
        let permit = self.routing.gate.enter();
        permit.check()?;
        self.inner.put(key, value).map_err(internal_error)
    }

    /// Traverse database column with the given callback function.
    pub fn full_traverse<F>(&self, col: Col, callback: &mut F) -> Result<()>
    where
        F: FnMut(&[u8], &[u8]) -> Result<()>,
    {
        let mut options = ReadOptions::default();
        options.set_total_order_seek(true);
        let iter = self.iter_opt(col, IteratorMode::Start, &options)?;
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
        let mut options = ReadOptions::default();
        options.set_total_order_seek(true);
        let iter = self.iter_opt(col, mode, &options)?;
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

        let permit = self.routing.gate.enter();
        RocksDBTransaction {
            generation: self.generation(),
            permit,
            inner: self.inner.transaction(&write_options, &transaction_options),
        }
    }

    /// Construct `RocksDBWriteBatch` with default option.
    pub fn new_write_batch(&self) -> RocksDBWriteBatch {
        let permit = self.routing.gate.enter();
        RocksDBWriteBatch {
            generation: self.generation(),
            permit: Arc::new(permit),
            db: Arc::clone(&self.inner),
            inner: WriteBatch::default(),
        }
    }

    /// Prepare and synchronize metadata against a view with all ordinary writers
    /// drained. Returns false when the writer wait budget expires or `prepare`
    /// defers the operation. The callback must use the supplied batch and must
    /// not start another writer; returning false discards the prepared batch.
    pub fn write_when_idle(
        &self,
        timeout: std::time::Duration,
        prepare: impl FnOnce(&RocksDBSnapshot, &mut RocksDBWriteBatch) -> Result<bool>,
    ) -> Result<bool> {
        let _exclusive = self
            .routing
            .collector
            .lock()
            .expect("collector lock poisoned");
        self.routing.gate.check()?;
        let Ok(_pause) = self.routing.gate.pause(timeout) else {
            return Ok(false);
        };
        let mut batch = RocksDBWriteBatch {
            generation: self.generation(),
            permit: Arc::new(self.routing.gate.enter_paused()),
            db: Arc::clone(&self.inner),
            inner: WriteBatch::default(),
        };
        if !prepare(&self.get_snapshot(), &mut batch)? {
            return Ok(false);
        }
        self.write_sync(&batch)?;
        Ok(true)
    }

    /// Write batch into transaction db.
    pub fn write(&self, batch: &RocksDBWriteBatch) -> Result<()> {
        self.check_batch(batch)?;
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
        self.check_batch(batch)?;
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
        let generation = self.generation();
        let cf = generation.cf(col)?;
        self.inner.compact_range_cf(cf, start, end);
        Ok(())
    }

    /// Return `RocksDBSnapshot`.
    pub fn get_snapshot(&self) -> RocksDBSnapshot {
        // Keep the route lock until the sequence number is captured. Publication
        // cannot pair a new generation with an older database snapshot.
        let generation = self
            .routing
            .current
            .read()
            .expect("generation lock poisoned");
        unsafe {
            let snapshot = ffi::rocksdb_create_snapshot(self.inner.base_db_ptr());
            RocksDBSnapshot::new(&self.inner, Arc::clone(&generation), snapshot)
        }
    }

    pub(crate) fn generation(&self) -> Arc<Generation> {
        Arc::clone(
            &self
                .routing
                .current
                .read()
                .expect("generation lock poisoned"),
        )
    }

    fn check_batch(&self, batch: &RocksDBWriteBatch) -> Result<()> {
        if !Arc::ptr_eq(&self.inner, &batch.db) {
            return Err(internal_error("write batch belongs to another database"));
        }
        batch.permit.check()
    }

    /// Return rocksdb `OptimisticTransactionDB`.
    pub fn inner(&self) -> Arc<OptimisticTransactionDB> {
        Arc::clone(&self.inner)
    }

    /// Create a non-payload column family with exclusive access (used by migrations).
    pub fn create_cf(&mut self, col: Col) -> Result<()> {
        if PAYLOAD_COLUMNS.contains(&col) {
            return Err(internal_error(
                "payload columns are managed by freezer generations",
            ));
        }
        let routing = Arc::get_mut(&mut self.routing)
            .ok_or_else(|| internal_error("create_cf requires an exclusive database"))?;
        let generation = routing.current.get_mut().expect("generation lock poisoned");
        let generation = Arc::get_mut(generation)
            .ok_or_else(|| internal_error("create_cf requires released read views and writers"))?;
        if generation.columns.contains_key(col) {
            return Err(internal_error(format!("column {col} already exists")));
        }
        let column = self
            .inner
            .create_owned_cf(col, &Options::default())
            .map_err(internal_error)?;
        generation.columns.insert(col.to_owned(), column);
        Ok(())
    }

    /// Drop a non-payload column family with exclusive access (used by migrations).
    pub fn drop_cf(&mut self, col: Col) -> Result<()> {
        if PAYLOAD_COLUMNS.contains(&col) {
            return Err(internal_error(
                "payload columns are managed by freezer generations",
            ));
        }
        let routing = Arc::get_mut(&mut self.routing)
            .ok_or_else(|| internal_error("drop_cf requires an exclusive database"))?;
        let generation = routing.current.get_mut().expect("generation lock poisoned");
        let generation = Arc::get_mut(generation)
            .ok_or_else(|| internal_error("drop_cf requires released read views and writers"))?;
        let column = generation
            .columns
            .get(col)
            .ok_or_else(|| internal_error(format!("column {col} not found")))?;
        column.drop_from_database().map_err(internal_error)?;
        generation.columns.remove(col);
        checkpoint_retirement(&self.inner).inspect_err(|_| routing.gate.fail())
    }

    /// "rocksdb.estimate-num-keys" - returns estimated number of total keys in
    /// the active and unflushed immutable memtables and storage.
    pub fn estimate_num_keys_cf(&self, col: Col) -> Result<Option<u64>> {
        let generation = self.generation();
        let cf = generation.cf(col)?;
        self.inner
            .property_int_value_cf(cf, PROPERTY_NUM_KEYS)
            .map_err(internal_error)
    }

    /// Read a property from each currently routed logical column family.
    /// Logical labels remain stable when physical payload generations change.
    pub fn property_int_values(&self, property: &str) -> Vec<(String, Result<Option<u64>>)> {
        self.generation()
            .columns
            .iter()
            .map(|(logical, column)| {
                (
                    logical.clone(),
                    self.inner
                        .property_int_value_cf(column, property)
                        .map_err(internal_error),
                )
            })
            .collect()
    }
}
