//! **Deprecated**, Please use [ckb-indexer](https://github.com/nervosnetwork/ckb-indexer) as an alternate solution.
#![allow(missing_docs)]

mod migrations;
mod store;
mod types;

pub use store::{DefaultIndexerStore, IndexerStore};
pub use types::{CellTransaction, LiveCell, TransactionPoint};
