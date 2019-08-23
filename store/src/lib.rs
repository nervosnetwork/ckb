mod config;
pub mod data_loader_wrapper;
mod db;
mod snapshot;
mod store;
mod transaction;

pub use config::StoreConfig;
pub use db::ChainDB;
pub use snapshot::StoreSnapshot;
pub use store::ChainStore;
pub use transaction::StoreTransaction;

use ckb_db::Col;
use ckb_types::{
    bytes::Bytes,
    core::{HeaderView, TransactionView, UncleBlockVecView},
    packed::{Byte32, ProposalShortIdVec},
};
use lru_cache::LruCache;
use std::cell::RefCell;

pub const COLUMNS: u32 = 12;
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

const META_TIP_HEADER_KEY: &[u8] = b"TIP_HEADER";
const META_CURRENT_EPOCH_KEY: &[u8] = b"CURRENT_EPOCH";

thread_local! {
    pub static HEADER_CACHE: RefCell<LruCache<Byte32, HeaderView>> = RefCell::new(LruCache::new(4096));
    pub static CELL_DATA_CACHE: RefCell<LruCache<(Byte32, u32), Bytes>> = RefCell::new(LruCache::new(128));
    pub static BLOCK_PROPOSALS_CACHE: RefCell<LruCache<Byte32, ProposalShortIdVec>> = RefCell::new(LruCache::new(30));
    pub static BLOCK_TX_HASHES_CACHE: RefCell<LruCache<Byte32, Vec<Byte32>>> = RefCell::new(LruCache::new(20));
    pub static BLOCK_UNCLES_CACHE: RefCell<LruCache<Byte32, UncleBlockVecView>> = RefCell::new(LruCache::new(10));
    pub static CELLBASE_CACHE: RefCell<LruCache<Byte32, TransactionView>> = RefCell::new(LruCache::new(20));
}
