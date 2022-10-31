//! DB with ttl support wrapper

use crate::{internal_error, Result};
use rocksdb::ops::{DropCF, GetColumnFamilys, GetPinnedCF, GetPropertyCF, OpenCF, PutCF};
use rocksdb::{
    ColumnFamilyDescriptor, DBPinnableSlice, DBWithTTL as RawDBWithTTL, Options, TTLOpenDescriptor,
};
use std::path::Path;

const PROPERTY_NUM_KEYS: &str = "rocksdb.estimate-num-keys";

/// DB with ttl support wrapper
///
/// TTL is accepted in seconds
/// If TTL is non positive or not provided, the behaviour is TTL = infinity
/// (int32_t)Timestamp(creation) is suffixed to values in Put internally
/// Expired TTL values are deleted in compaction only:(Timestamp+ttl<time_now)
/// Get/Iterator may return expired entries(compaction not run on them yet)
/// Different TTL may be used during different Opens
/// Example: Open1 at t=0 with ttl=4 and insert k1,k2, close at t=2. Open2 at t=3 with ttl=5. Now k1,k2 should be deleted at t>=5
/// read_only=true opens in the usual read-only mode. Compactions will not be triggered(neither manual nor automatic), so no expired entries removed
#[derive(Debug)]
pub struct DBWithTTL {
    pub(crate) inner: RawDBWithTTL,
}

impl DBWithTTL {
    /// Open a database with ttl support.
    pub fn open_cf<P, I, N>(path: P, cf_names: I, ttl: i32) -> Result<Self>
    where
        P: AsRef<Path>,
        I: IntoIterator<Item = N>,
        N: Into<String>,
    {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        opts.set_keep_log_file_num(10);

        let cf_descriptors: Vec<_> = cf_names
            .into_iter()
            .map(|name| ColumnFamilyDescriptor::new(name, Options::default()))
            .collect();

        let descriptor = TTLOpenDescriptor::by_default(ttl);
        let inner = RawDBWithTTL::open_cf_descriptors_with_descriptor(
            &opts,
            path,
            cf_descriptors,
            descriptor,
        )
        .map_err(|err| internal_error(format!("failed to open database: {}", err)))?;
        Ok(DBWithTTL { inner })
    }

    /// Return the value associated with a key using RocksDB's PinnableSlice from the given column
    /// so as to avoid unnecessary memory copy.
    pub fn get_pinned(&self, col: &str, key: &[u8]) -> Result<Option<DBPinnableSlice>> {
        let cf = self
            .inner
            .cf_handle(col)
            .ok_or_else(|| internal_error(format!("column {} not found", col)))?;
        self.inner.get_pinned_cf(cf, &key).map_err(internal_error)
    }

    /// Insert a value into the database under the given key.
    pub fn put<K, V>(&self, col: &str, key: K, value: V) -> Result<()>
    where
        K: AsRef<[u8]>,
        V: AsRef<[u8]>,
    {
        let cf = self
            .inner
            .cf_handle(col)
            .ok_or_else(|| internal_error(format!("column {} not found", col)))?;
        self.inner.put_cf(cf, key, value).map_err(internal_error)
    }

    /// Create a new column family for the database.
    pub fn create_cf_with_ttl(&mut self, col: &str, ttl: i32) -> Result<()> {
        let opts = Options::default();
        self.inner
            .create_cf_with_ttl(col, &opts, ttl)
            .map_err(internal_error)
    }

    /// Delete column family.
    pub fn drop_cf(&mut self, col: &str) -> Result<()> {
        self.inner.drop_cf(col).map_err(internal_error)
    }

    /// "rocksdb.estimate-num-keys" - returns estimated number of total keys in
    /// the active and unflushed immutable memtables and storage.
    pub fn estimate_num_keys_cf(&self, col: &str) -> Result<Option<u64>> {
        let cf = self
            .inner
            .cf_handle(col)
            .ok_or_else(|| internal_error(format!("column {} not found", col)))?;
        self.inner
            .property_int_value_cf(cf, PROPERTY_NUM_KEYS)
            .map_err(internal_error)
    }
}
