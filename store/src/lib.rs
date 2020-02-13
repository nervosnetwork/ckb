mod cache;
mod config;
pub mod data_loader_wrapper;
mod db;
mod snapshot;
mod store;
mod transaction;

pub use cache::StoreCache;
pub use ckb_db::{
    iter::{Direction as IterDirection, IteratorMode},
    WriteBatch,
};
pub use config::StoreConfig;
pub use db::ChainDB;
pub use snapshot::StoreSnapshot;
pub use store::ChainStore;
pub use transaction::StoreTransaction;

use ckb_db::Col;

pub const COLUMNS: u32 = 14;
pub const COLUMN_INDEX: Col = "0";
pub const COLUMN_BLOCK_HEADER: Col = "1";
pub const COLUMN_BLOCK_BODY: Col = "2";
pub const COLUMN_BLOCK_UNCLE: Col = "3";
pub const COLUMN_META: Col = "4";
pub const COLUMN_TRANSACTION_INFO: Col = "5";
pub const COLUMN_BLOCK_EXT: Col = "6";
pub const COLUMN_BLOCK_PROPOSAL_IDS: Col = "7";
pub const COLUMN_BLOCK_EPOCH: Col = "8";
pub const COLUMN_EPOCH: Col = "9";
pub const COLUMN_CELL_SET: Col = "10";
pub const COLUMN_UNCLES: Col = "11";
pub const COLUMN_PRUNE_MASK: Col = "12";
pub const COLUMN_PRUNED: Col = "13";

const META_TIP_HEADER_KEY: &[u8] = b"TIP_HEADER";
const META_CURRENT_EPOCH_KEY: &[u8] = b"CURRENT_EPOCH";
pub const META_PRUNING_EPOCH_KEY: &[u8] = b"PRUNING_EPOCH";
