//! RocksDB wrapper base on OptimisticTransactionDB
use crate::snapshot::RocksDBSnapshot;
use crate::transaction::RocksDBTransaction;
use crate::write_batch::RocksDBWriteBatch;
use crate::{internal_error, Result};
use ckb_app_config::DBConfig;
use ckb_db_schema::Col;
use ckb_logger::info;
use rocksdb::ops::{
    CompactRangeCF, CreateCF, DropCF, GetColumnFamilys, GetPinned, GetPinnedCF, IterateCF, OpenCF,
    Put, SetOptions, WriteOps,
};
use rocksdb::{
    ffi, ColumnFamily, ColumnFamilyDescriptor, DBPinnableSlice, FullOptions, IteratorMode,
    OptimisticTransactionDB, OptimisticTransactionOptions, Options, WriteBatch, WriteOptions,
};
use std::path::Path;
use std::sync::Arc;

/// RocksDB wrapper base on OptimisticTransactionDB
///
/// https://github.com/facebook/rocksdb/wiki/Transactions#optimistictransactiondb
#[derive(Clone)]
pub struct RocksDB {
    pub(crate) inner: Arc<OptimisticTransactionDB>,
}

const DEFAULT_CACHE_SIZE: usize = 128 << 20;

impl RocksDB {
    pub(crate) fn open_with_check(config: &DBConfig, columns: u32) -> Result<Self> {
        let cf_names: Vec<_> = (0..columns).map(|c| c.to_string()).collect();

        let (mut opts, cf_descriptors) = if let Some(ref file) = config.options_file {
            let cache_size = match config.cache_size {
                Some(0) => None,
                Some(size) => Some(size),
                None => Some(DEFAULT_CACHE_SIZE),
            };

            let mut full_opts =
                FullOptions::load_from_file(file, cache_size, false).map_err(|err| {
                    internal_error(format!("failed to load the options file: {}", err))
                })?;
            let cf_names_str: Vec<&str> = cf_names.iter().map(|s| s.as_str()).collect();
            full_opts
                .complete_column_families(&cf_names_str, false)
                .map_err(|err| {
                    internal_error(format!("failed to check all column families: {}", err))
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

        opts.create_if_missing(true);
        opts.create_missing_column_families(true);

        let db = OptimisticTransactionDB::open_cf_descriptors(&opts, &config.path, cf_descriptors)
            .map_err(|err| internal_error(format!("failed to open database: {}", err)))?;

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

    /// Repairer does best effort recovery to recover as much data as possible
    /// after a disaster without compromising consistency.
    /// It does not guarantee bringing the database to a time consistent state.
    /// Note: Currently there is a limitation that un-flushed column families will be lost after repair.
    /// This would happen even if the DB is in healthy state.
    pub fn repair<P: AsRef<Path>>(path: P) -> Result<()> {
        let repair_opts = Options::default();
        OptimisticTransactionDB::repair(repair_opts, path)
            .map_err(|err| internal_error(format!("failed to repair database: {}", err)))
    }

    /// Open a database with the given configuration and columns count.
    pub fn open(config: &DBConfig, columns: u32) -> Self {
        Self::open_with_check(config, columns).unwrap_or_else(|err| panic!("{}", err))
    }

    /// Open a database in the given directory with the default configuration and columns count.
    pub fn open_in<P: AsRef<Path>>(path: P, columns: u32) -> Self {
        let config = DBConfig {
            path: path.as_ref().to_path_buf(),
            ..Default::default()
        };
        Self::open_with_check(&config, columns).unwrap_or_else(|err| panic!("{}", err))
    }

    /// Set appropriate parameters for bulk loading.
    pub fn prepare_for_bulk_load_open<P: AsRef<Path>>(
        path: P,
        columns: u32,
    ) -> Result<Option<Self>> {
        let mut opts = Options::default();

        opts.create_missing_column_families(true);
        opts.set_prepare_for_bulk_load();

        let cfnames: Vec<_> = (0..columns).map(|c| c.to_string()).collect();
        let cf_options: Vec<&str> = cfnames.iter().map(|n| n as &str).collect();

        OptimisticTransactionDB::open_cf(&opts, path, cf_options).map_or_else(
            |err| {
                let err_str = err.as_ref();
                if err_str.starts_with("Invalid argument:")
                    && err_str.ends_with("does not exist (create_if_missing is false)")
                {
                    Ok(None)
                } else if err_str.starts_with("Corruption:") {
                    info!(
                        "DB corrupted: {}.\n\
                        Try ckb db-repair command to repair DB.\n\
                        Note: Currently there is a limitation that un-flushed column families will be lost after repair.\
                        This would happen even if the DB is in healthy state.\n\
                        See https://github.com/facebook/rocksdb/wiki/RocksDB-Repairer for detail",
                        err_str
                    );
                    Err(internal_error(err_str))
                } else {
                    Err(internal_error(format!(
                        "failed to open the database: {}",
                        err
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
    pub fn get_pinned(&self, col: Col, key: &[u8]) -> Result<Option<DBPinnableSlice>> {
        let cf = cf_handle(&self.inner, col)?;
        self.inner.get_pinned_cf(cf, key).map_err(internal_error)
    }

    /// Return the value associated with a key using RocksDB's PinnableSlice from the default column
    /// so as to avoid unnecessary memory copy.
    pub fn get_pinned_default(&self, key: &[u8]) -> Result<Option<DBPinnableSlice>> {
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
}

#[inline]
pub(crate) fn cf_handle(db: &OptimisticTransactionDB, col: Col) -> Result<&ColumnFamily> {
    db.cf_handle(col)
        .ok_or_else(|| internal_error(format!("column {} not found", col)))
}
