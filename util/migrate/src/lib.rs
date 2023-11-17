//! CKB migrate.
//!
//! ckb migrate help to migrate CKB's data on schema change.

// declare here for mute ./devtools/ci/check-cargotoml.sh error
extern crate num_cpus;

pub mod migrate;
mod migrations;
#[cfg(test)]
mod tests;
