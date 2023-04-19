//! # The Store Library
//!
//! This Library contains the `ChainStore` traits
//! which provides chain data store interface

mod cache;
mod cell;
pub mod data_loader_wrapper;
mod db;
mod snapshot;
mod store;
mod transaction;
mod write_batch;

#[cfg(test)]
mod tests;

pub use cache::StoreCache;
pub use cell::{attach_block_cell, detach_block_cell};
pub use db::ChainDB;
pub use snapshot::StoreSnapshot;
pub use store::ChainStore;
pub use transaction::StoreTransaction;
pub use write_batch::StoreWriteBatch;

pub use ckb_freezer::Freezer;
