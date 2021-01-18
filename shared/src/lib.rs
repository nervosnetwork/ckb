//! TODO(doc): @quake

// num_cpus is used in proc_macro
// declare here for mute ./devtools/ci/check-cargotoml.sh error
extern crate num_cpus;

mod migrations;
pub mod shared;

pub use ckb_snapshot::{Snapshot, SnapshotMgr};
