//! CKB's built-in indexer-r, based on relational database,
//! which shares data with the ckb node by creating secondary db instances.

mod indexer;
mod service;
mod store;

pub use service::{IndexerRHandle, IndexerRService};
