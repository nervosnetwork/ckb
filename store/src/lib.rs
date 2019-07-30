mod config;
pub mod data_loader_wrapper;
mod db;
mod store;
mod transaction;

pub use config::StoreConfig;
pub use db::ChainDB;
pub use store::ChainStore;
pub use transaction::StoreTransaction;

use ckb_core::header::Header;
use ckb_core::Bytes;
use ckb_db::Col;
use ckb_util::Mutex;
use lazy_static::lazy_static;
use lru_cache::LruCache;
use numext_fixed_hash::H256;

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
    static ref HEADER_CACHE: Mutex<LruCache<H256, Header>> = { Mutex::new(LruCache::new(4096)) };
}

lazy_static! {
    static ref CELL_DATA_CACHE: Mutex<LruCache<(H256, u32), Bytes>> =
        { Mutex::new(LruCache::new(128)) };
}
