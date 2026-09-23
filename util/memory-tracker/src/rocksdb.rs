use ckb_db::internal::ops::{GetColumnFamilys, GetProperty, GetPropertyCF};

#[allow(dead_code)]
#[derive(Debug, Clone)]
enum PropertyValue<T> {
    Value(T),
    Null,
    Error(String),
}

impl PropertyValue<u64> {
    pub(crate) fn as_i64(&self) -> i64 {
        match self {
            Self::Value(v) => *v as i64,
            Self::Null => -1,
            Self::Error(_) => -2,
        }
    }
}

impl<T> From<Result<Option<T>, String>> for PropertyValue<T> {
    fn from(res: Result<Option<T>, String>) -> Self {
        match res {
            Ok(Some(v)) => Self::Value(v),
            Ok(None) => Self::Null,
            Err(e) => Self::Error(e),
        }
    }
}

/// A trait which used to track the RocksDB memory usage.
///
/// References: [Memory usage in RocksDB](https://github.com/facebook/rocksdb/wiki/Memory-usage-in-RocksDB)
pub trait TrackRocksDBMemory {
    /// Gather memory statistics through [ckb-metrics](../../ckb_metrics/index.html)
    fn gather_memory_stats(&self) {
        self.gather_int_values("estimate-table-readers-mem");
        self.gather_int_values("size-all-mem-tables");
        self.gather_int_values("cur-size-all-mem-tables");
        self.gather_int_values("block-cache-capacity");
        self.gather_int_values("block-cache-usage");
        self.gather_int_values("block-cache-pinned-usage");
    }

    /// Gather integer values through [ckb-metrics](../../ckb_metrics/index.html)
    fn gather_int_values(&self, _: &str) {}
}

pub(crate) struct DummyRocksDB;

/// Memory statistics for the current logical columns of a routed CKB database.
pub struct RocksDBMemoryTracker(pub ckb_db::RocksDB);

impl TrackRocksDBMemory for RocksDBMemoryTracker {
    fn gather_int_values(&self, key: &str) {
        let Some(metrics) = ckb_metrics::handle() else {
            return;
        };
        for (name, value) in self.0.property_int_values(&format!("rocksdb.{key}")) {
            let value: PropertyValue<u64> = value.map_err(|error| error.to_string()).into();
            metrics
                .ckb_sys_mem_rocksdb
                .with_label_values(&[key, &name])
                .set(value.as_i64());
        }
    }
}

impl TrackRocksDBMemory for DummyRocksDB {}

impl<RocksDB> TrackRocksDBMemory for RocksDB
where
    RocksDB: GetColumnFamilys + GetProperty + GetPropertyCF,
{
    fn gather_int_values(&self, key: &str) {
        let mut values = Vec::new();
        for (cf_name, cf) in self.get_cfs() {
            let value_col: PropertyValue<u64> = self
                .property_int_value_cf(cf, &format!("rocksdb.{key}"))
                .map_err(|err| format!("{err}"))
                .into();
            if let Some(metrics) = ckb_metrics::handle() {
                metrics
                    .ckb_sys_mem_rocksdb
                    .with_label_values(&[key, cf_name])
                    .set(value_col.as_i64());
            }
            values.push(value_col);
        }
    }
}
