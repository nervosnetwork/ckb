//! Legacy numeric RocksDB column family names.

use crate::Col;

/// Total legacy column family number.
pub const COLUMNS: u32 = 19;

/// Legacy numeric column family names in creation/open order.
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

/// Legacy column for main chain block number to block hash mapping.
pub const COLUMN_INDEX: Col = "0";
/// Legacy column for block headers keyed by block hash.
pub const COLUMN_BLOCK_HEADER: Col = "1";
/// Legacy column for block bodies keyed by block hash and transaction index.
pub const COLUMN_BLOCK_BODY: Col = "2";
/// Legacy column for block uncles keyed by block hash.
pub const COLUMN_BLOCK_UNCLE: Col = "3";
/// Legacy column for metadata.
pub const COLUMN_META: Col = "4";
/// Legacy column for transaction metadata.
pub const COLUMN_TRANSACTION_INFO: Col = "5";
/// Legacy column for block extension data.
pub const COLUMN_BLOCK_EXT: Col = "6";
/// Legacy column for block proposal IDs.
pub const COLUMN_BLOCK_PROPOSAL_IDS: Col = "7";
/// Legacy column for block epoch data.
pub const COLUMN_BLOCK_EPOCH: Col = "8";
/// Legacy column for epoch data.
pub const COLUMN_EPOCH: Col = "9";
/// Legacy column for live cell metadata.
pub const COLUMN_CELL: Col = "10";
/// Legacy column for uncle blocks.
pub const COLUMN_UNCLES: Col = "11";
/// Legacy column for cell data.
pub const COLUMN_CELL_DATA: Col = "12";
/// Legacy column for block number and hash mappings.
pub const COLUMN_NUMBER_HASH: Col = "13";
/// Legacy column for cell data hash mappings.
pub const COLUMN_CELL_DATA_HASH: Col = "14";
/// Legacy column for optional block extensions.
pub const COLUMN_BLOCK_EXTENSION: Col = "15";
/// Legacy column for chain root MMR data.
pub const COLUMN_CHAIN_ROOT_MMR: Col = "16";
/// Legacy column for block filter data.
pub const COLUMN_BLOCK_FILTER: Col = "17";
/// Legacy column for block filter hashes.
pub const COLUMN_BLOCK_FILTER_HASH: Col = "18";
