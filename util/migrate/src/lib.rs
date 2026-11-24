//! CKB migrate.
//!
//! ckb migrate help to migrate CKB's data on schema change.

// declare here for mute ./devtools/ci/check-cargotoml.sh error
extern crate num_cpus;

mod legacy_store;
pub mod migrate;
mod migrations;
/// Offline RocksDB SST rebuild migration.
pub mod sst_rebuild;
#[cfg(test)]
mod tests;
