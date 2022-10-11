//! CKB's built-in indexer, which shares data with the ckb node by creating secondary db instances.

pub(crate) mod error;
pub(crate) mod indexer;
pub(crate) mod pool;
pub(crate) mod store;

/// The indexer service.
pub mod service;

pub use service::{IndexerHandle, IndexerService};
