//! TODO(doc): @yangby-cryptape
use ckb_db::internal::ops::{GetColumnFamilys, GetProperty, GetPropertyCF};
use ckb_metrics::metrics;

use crate::utils::{sum_int_values, PropertyValue};

/// TODO(doc): @yangby-cryptape
// Ref: https://github.com/facebook/rocksdb/wiki/Memory-usage-in-RocksDB
pub struct RocksDBMemoryStatistics {
    /// TODO(doc): @yangby-cryptape
    pub estimate_table_readers_mem: PropertyValue<u64>,
    /// TODO(doc): @yangby-cryptape
    pub size_all_mem_tables: PropertyValue<u64>,
    /// TODO(doc): @yangby-cryptape
    pub cur_size_all_mem_tables: PropertyValue<u64>,
    /// TODO(doc): @yangby-cryptape
    pub block_cache_capacity: PropertyValue<u64>,
    /// TODO(doc): @yangby-cryptape
    pub block_cache_usage: PropertyValue<u64>,
    /// TODO(doc): @yangby-cryptape
    pub block_cache_pinned_usage: PropertyValue<u64>,
}

/// TODO(doc): @yangby-cryptape
pub trait TrackRocksDBMemory {
    /// TODO(doc): @yangby-cryptape
    fn gather_memory_stats(&self) -> RocksDBMemoryStatistics {
        let estimate_table_readers_mem = self.gather_int_values("estimate-table-readers-mem");
        let size_all_mem_tables = self.gather_int_values("size-all-mem-tables");
        let cur_size_all_mem_tables = self.gather_int_values("cur-size-all-mem-tables");
        let block_cache_capacity = self.gather_int_values("block-cache-capacity");
        let block_cache_usage = self.gather_int_values("block-cache-usage");
        let block_cache_pinned_usage = self.gather_int_values("block-cache-pinned-usage");
        RocksDBMemoryStatistics {
            estimate_table_readers_mem,
            size_all_mem_tables,
            cur_size_all_mem_tables,
            block_cache_capacity,
            block_cache_usage,
            block_cache_pinned_usage,
        }
    }
    /// TODO(doc): @yangby-cryptape
    fn gather_int_values(&self, key: &str) -> PropertyValue<u64>;
}

/// TODO(doc): @yangby-cryptape
pub struct DummyRocksDB;

impl TrackRocksDBMemory for DummyRocksDB {
    fn gather_int_values(&self, _: &str) -> PropertyValue<u64> {
        PropertyValue::Null
    }
}

impl<RocksDB> TrackRocksDBMemory for RocksDB
where
    RocksDB: GetColumnFamilys + GetProperty + GetPropertyCF,
{
    fn gather_int_values(&self, key: &str) -> PropertyValue<u64> {
        let mut values = Vec::new();
        for (cf_name, cf) in self.get_cfs() {
            let value_col: PropertyValue<u64> = self
                .property_int_value_cf(cf, &format!("rocksdb.{}", key))
                .map_err(|err| format!("{}", err))
                .into();
            metrics!(gauge, "ckb-sys.mem.rocksdb", value_col.as_i64(), "type" => key.to_owned(), "cf" => cf_name.to_owned());
            values.push(value_col);
        }
        sum_int_values(&values)
    }
}
