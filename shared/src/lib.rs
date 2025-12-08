//! Shared blockchain data and services.
//!
//! This crate provides shared access to blockchain data and consensus components
//! across different parts of the CKB node, including chain state, consensus, and
//! transaction pool integration.

// num_cpus is used in proc_macro
pub mod chain_services_builder;
pub mod shared;
pub mod shared_builder;

pub use chain_services_builder::ChainServicesBuilder;
pub use ckb_snapshot::{Snapshot, SnapshotMgr};
pub use shared::Shared;
pub use shared_builder::{SharedBuilder, SharedPackage};
pub mod block_status;
pub mod types;

pub use types::header_map::HeaderMap;
pub use types::{HeaderIndex, HeaderIndexView};
