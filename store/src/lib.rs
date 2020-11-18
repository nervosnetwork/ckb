//! TODO(doc): @quake
mod cache;
mod cell;
pub mod data_loader_wrapper;
mod db;
mod snapshot;
mod store;
mod transaction;
mod write_batch;

pub use cache::StoreCache;
pub use cell::{attach_block_cell, detach_block_cell};
pub use db::ChainDB;
pub use snapshot::StoreSnapshot;
pub use store::ChainStore;
pub use transaction::StoreTransaction;
pub use write_batch::StoreWriteBatch;

use ckb_db::Col;

/// TODO(doc): @quake
pub const COLUMNS: u32 = 13;
/// TODO(doc): @quake
pub const COLUMN_INDEX: Col = "0";
/// TODO(doc): @quake
pub const COLUMN_BLOCK_HEADER: Col = "1";
/// TODO(doc): @quake
pub const COLUMN_BLOCK_BODY: Col = "2";
/// TODO(doc): @quake
pub const COLUMN_BLOCK_UNCLE: Col = "3";
/// TODO(doc): @quake
pub const COLUMN_META: Col = "4";
/// TODO(doc): @quake
pub const COLUMN_TRANSACTION_INFO: Col = "5";
/// TODO(doc): @quake
pub const COLUMN_BLOCK_EXT: Col = "6";
/// TODO(doc): @quake
pub const COLUMN_BLOCK_PROPOSAL_IDS: Col = "7";
/// TODO(doc): @quake
pub const COLUMN_BLOCK_EPOCH: Col = "8";
/// TODO(doc): @quake
pub const COLUMN_EPOCH: Col = "9";
/// TODO(doc): @quake
pub const COLUMN_CELL: Col = "10";
/// TODO(doc): @quake
pub const COLUMN_UNCLES: Col = "11";
/// TODO(doc): @quake
pub const COLUMN_CELL_DATA: Col = "12";

/// TODO(doc): @quake
pub const META_TIP_HEADER_KEY: &[u8] = b"TIP_HEADER";
/// TODO(doc): @quake
pub const META_CURRENT_EPOCH_KEY: &[u8] = b"CURRENT_EPOCH";
