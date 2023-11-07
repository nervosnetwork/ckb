//! CKB's built-in indexer-r, based on relational database,
//! which shares data with the ckb node by creating secondary db instances.

mod indexer;
mod indexer_handle;
mod service;
mod store;

pub use indexer_handle::{AsyncIndexerRHandle, IndexerRHandle};
pub use service::IndexerRService;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use indexer::AsyncIndexerR;
