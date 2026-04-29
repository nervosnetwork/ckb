//! Legacy numeric RocksDB column family names.

use crate::Col;

pub const COLUMNS: u32 = 19;

pub const COLUMN_FAMILIES: &[Col] = &[
    COLUMN_INDEX,
    COLUMN_BLOCK_HEADER,
    COLUMN_BLOCK_BODY,
    COLUMN_BLOCK_UNCLE,
    COLUMN_META,
    COLUMN_TRANSACTION_INFO,
    COLUMN_BLOCK_EXT,
    COLUMN_BLOCK_PROPOSAL_IDS,
    COLUMN_BLOCK_EPOCH,
    COLUMN_EPOCH,
    COLUMN_CELL,
    COLUMN_UNCLES,
    COLUMN_CELL_DATA,
    COLUMN_NUMBER_HASH,
    COLUMN_CELL_DATA_HASH,
    COLUMN_BLOCK_EXTENSION,
    COLUMN_CHAIN_ROOT_MMR,
    COLUMN_BLOCK_FILTER,
    COLUMN_BLOCK_FILTER_HASH,
];

const _: [(); COLUMNS as usize] = [(); COLUMN_FAMILIES.len()];

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
pub const COLUMN_CELL: Col = "10";
pub const COLUMN_UNCLES: Col = "11";
pub const COLUMN_CELL_DATA: Col = "12";
pub const COLUMN_NUMBER_HASH: Col = "13";
pub const COLUMN_CELL_DATA_HASH: Col = "14";
pub const COLUMN_BLOCK_EXTENSION: Col = "15";
pub const COLUMN_CHAIN_ROOT_MMR: Col = "16";
pub const COLUMN_BLOCK_FILTER: Col = "17";
pub const COLUMN_BLOCK_FILTER_HASH: Col = "18";
