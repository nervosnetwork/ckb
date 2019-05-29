mod flat_block_body;
mod lazy_load_cell_output;
mod store;

pub use lazy_load_cell_output::LazyLoadCellOutput;
pub use store::{ChainKVStore, ChainStore, StoreBatch, StoreConfig};

use ckb_db::Col;

pub const COLUMNS: u32 = 13;
pub const COLUMN_INDEX: Col = 0;
pub const COLUMN_BLOCK_HEADER: Col = 1;
pub const COLUMN_BLOCK_BODY: Col = 2;
pub const COLUMN_BLOCK_UNCLE: Col = 3;
pub const COLUMN_META: Col = 4;
pub const COLUMN_TRANSACTION_ADDR: Col = 5;
pub const COLUMN_EXT: Col = 6;
pub const COLUMN_BLOCK_TRANSACTION_ADDRESSES: Col = 7;
pub const COLUMN_BLOCK_PROPOSAL_IDS: Col = 8;
pub const COLUMN_CELL_META: Col = 9;
pub const COLUMN_BLOCK_EPOCH: Col = 10;
pub const COLUMN_EPOCH: Col = 11;
pub const COLUMN_CELL_SET: Col = 12;
