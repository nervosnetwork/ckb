use rocksdb::Options;
use serde_derive::Deserialize;
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize)]
pub struct DBConfig {
    pub backend: String, // "memory" or "rocksdb"
    pub rocksdb: Option<RocksDBConfig>,
}

/// RocksDB specific db configurations.
///
/// https://docs.rs/rocksdb/0.6.0/rocksdb/struct.Options.html
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RocksDBConfig {
    pub path: PathBuf,

    // Options in rust-rocksdb/src/db_options.rs
    pub increase_parallelism: Option<i32>,
    pub optimize_level_style_compaction: Option<usize>,
    pub create_if_missing: Option<bool>,
    pub create_missing_column_families: Option<bool>,
    // set_compression_type
    // set_compression_per_level
    // set_merge_operator
    // add_merge_operator
    // set_compaction_filter
    // set_comparator
    // set_prefix_extractor
    // add_comparator
    pub optimize_for_point_lookup: Option<u64>,
    pub set_max_open_files: Option<i32>,
    pub set_use_fsync: Option<bool>,
    pub set_bytes_per_sync: Option<u64>,
    pub set_allow_concurrent_memtable_write: Option<bool>,
    pub set_use_direct_reads: Option<bool>,
    pub set_use_direct_io_for_flush_and_compaction: Option<bool>,
    // deprecated: pub set_allow_os_buffer: Option<bool>,
    pub set_table_cache_num_shard_bits: Option<i32>,
    pub set_min_write_buffer_number: Option<i32>,
    pub set_max_write_buffer_number: Option<i32>,
    pub set_write_buffer_size: Option<usize>,
    pub set_max_bytes_for_level_base: Option<u64>,
    pub set_max_bytes_for_level_multiplier: Option<f64>,
    pub set_max_manifest_file_size: Option<usize>,
    pub set_target_file_size_base: Option<u64>,
    pub set_min_write_buffer_number_to_merge: Option<i32>,
    pub set_level_zero_file_num_compaction_trigger: Option<i32>,
    pub set_level_zero_slowdown_writes_trigger: Option<i32>,
    pub set_level_zero_stop_writes_trigger: Option<i32>,
    // set_compaction_style
    pub set_max_background_compactions: Option<i32>,
    pub set_max_background_flushes: Option<i32>,
    pub set_disable_auto_compactions: Option<bool>,
    // set_block_based_table_factory
    pub set_report_bg_io_stats: Option<bool>,
    // set_wal_recovery_mode
    pub enable_statistics: Option<String>,
    pub set_stats_dump_period_sec: Option<u32>,
    pub set_advise_random_on_open: Option<bool>,
    pub set_num_levels: Option<i32>,
}

/// Macro to set Rocksdb options.
///
/// e.g.
/// ```
/// set_rocksdb_options!(self, opts, increase_parallelism, optimize_level_style_compaction, ...);
/// ```
/// =>
/// ```
/// if let Some(x) = self.increase_parallelism {
///     opts.increase_parallelism(x);
/// }
/// if let Some(x) = self.optimize_level_style_compaction {
///     opts.optimize_level_style_compaction(x);
/// }
/// ...
/// ```
macro_rules! set_rocksdb_options {
    ($self:ident, $opts:ident, $($name:tt),*) => {{
        $( if let Some(x) = $self.$name { $opts.$name(x); } )*
    }}
}

impl RocksDBConfig {
    pub fn to_db_options(&self) -> Options {
        let mut opts = Options::default();

        opts.create_if_missing(self.create_if_missing.unwrap_or(true));
        opts.create_missing_column_families(self.create_missing_column_families.unwrap_or(true));

        if let Some(_) = self.enable_statistics {
            opts.enable_statistics();
        }

        set_rocksdb_options!(
            self,
            opts,
            increase_parallelism,
            optimize_level_style_compaction,
            optimize_for_point_lookup,
            set_max_open_files,
            set_use_fsync,
            set_bytes_per_sync,
            set_allow_concurrent_memtable_write,
            set_use_direct_reads,
            set_use_direct_io_for_flush_and_compaction,
            set_table_cache_num_shard_bits,
            set_min_write_buffer_number,
            set_max_write_buffer_number,
            set_write_buffer_size,
            set_max_bytes_for_level_base,
            set_max_bytes_for_level_multiplier,
            set_max_manifest_file_size,
            set_target_file_size_base,
            set_min_write_buffer_number_to_merge,
            set_level_zero_file_num_compaction_trigger,
            set_level_zero_slowdown_writes_trigger,
            set_level_zero_stop_writes_trigger,
            set_max_background_compactions,
            set_max_background_flushes,
            set_disable_auto_compactions,
            set_report_bg_io_stats,
            set_stats_dump_period_sec,
            set_advise_random_on_open,
            set_num_levels
        );
        opts
    }
}
