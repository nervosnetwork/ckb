//! CKB's built-in rich-indexer, based on relational database,
//! which shares data with the ckb node by creating secondary db instances.

mod indexer;
mod indexer_handle;
mod service;
mod store;

pub use indexer_handle::{AsyncRichIndexerHandle, RichIndexerHandle};
pub use service::RichIndexerService;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use indexer::AsyncRichIndexer;
