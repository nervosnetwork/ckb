mod config;
pub mod data_loader_wrapper;
mod db;
mod store;
mod transaction;

pub use config::StoreConfig;
pub use db::ChainDB;
pub use store::ChainStore;
pub use transaction::StoreTransaction;

use ckb_db::Col;
use ckb_types::{
    bytes::Bytes,
    core::{BlockExt, HeaderView, TransactionView, UncleBlockVecView},
    packed::{Byte32, ProposalShortIdVec},
};
use ckb_util::Mutex;
use lazy_static::lazy_static;
use lru_cache::LruCache;
use std::sync::atomic::{AtomicBool, Ordering};

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

lazy_static! {
    static ref CACHE_ENABLE: AtomicBool = AtomicBool::new(true);
    static ref HEADER_CACHE: Mutex<LruCache<Byte32, HeaderView>> =
        { Mutex::new(LruCache::new(4096)) };
    static ref CELL_DATA_CACHE: Mutex<LruCache<(Byte32, u32), Bytes>> =
        { Mutex::new(LruCache::new(128)) };
    static ref BLOCK_PROPOSALS_CACHE: Mutex<LruCache<Byte32, ProposalShortIdVec>> =
        { Mutex::new(LruCache::new(30)) };
    static ref BLOCK_TX_HASHES_CACHE: Mutex<LruCache<Byte32, Vec<Byte32>>> =
        { Mutex::new(LruCache::new(20)) };
    static ref BLOCK_EXT_CACHE: Mutex<LruCache<Byte32, BlockExt>> =
        { Mutex::new(LruCache::new(20)) };
    static ref BLOCK_UNCLES_CACHE: Mutex<LruCache<Byte32, UncleBlockVecView>> =
        { Mutex::new(LruCache::new(10)) };
    static ref CELLBASE_CACHE: Mutex<LruCache<Byte32, TransactionView>> =
        { Mutex::new(LruCache::new(20)) };
}

pub fn cache_enable() -> bool {
    CACHE_ENABLE.load(Ordering::SeqCst)
}

pub fn set_cache_enable(enable: bool) {
    CACHE_ENABLE.store(enable, Ordering::SeqCst)
}
