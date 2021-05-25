//! The schema include constants define the low level database column families.

/// Column families alias type
pub type Col = &'static str;
/// Total column number
pub const COLUMNS: u32 = 15;
/// Column store chain index
pub const COLUMN_INDEX: Col = "0";
/// Column store block's header
pub const COLUMN_BLOCK_HEADER: Col = "1";
/// Column store block's body
pub const COLUMN_BLOCK_BODY: Col = "2";
/// Column store block's uncle and uncles’ proposal zones
pub const COLUMN_BLOCK_UNCLE: Col = "3";
/// Column store meta data
pub const COLUMN_META: Col = "4";
/// Column store transaction extra information
pub const COLUMN_TRANSACTION_INFO: Col = "5";
/// Column store block extra information
pub const COLUMN_BLOCK_EXT: Col = "6";
/// Column store block's proposal ids
pub const COLUMN_BLOCK_PROPOSAL_IDS: Col = "7";
/// Column store indicates track block epoch
pub const COLUMN_BLOCK_EPOCH: Col = "8";
/// Column store indicates track block epoch
pub const COLUMN_EPOCH: Col = "9";
/// Column store cell
pub const COLUMN_CELL: Col = "10";
/// Column store main chain consensus include uncles
///
/// https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0020-ckb-consensus-protocol/0020-ckb-consensus-protocol.md#specification
pub const COLUMN_UNCLES: Col = "11";
/// Column store cell data
pub const COLUMN_CELL_DATA: Col = "12";
/// Column store block number-hash pair
pub const COLUMN_NUMBER_HASH: Col = "13";

/// Column store block extension data
pub const COLUMN_BLOCK_EXTENSION: Col = "14";

/// META_TIP_HEADER_KEY tracks the latest known best block header
pub const META_TIP_HEADER_KEY: &[u8] = b"TIP_HEADER";
/// META_CURRENT_EPOCH_KEY tracks the latest known epoch
pub const META_CURRENT_EPOCH_KEY: &[u8] = b"CURRENT_EPOCH";

/// CHAIN_SPEC_HASH_KEY tracks the hash of chain spec which created current database
pub const CHAIN_SPEC_HASH_KEY: &[u8] = b"chain-spec-hash";
/// MIGRATION_VERSION_KEY tracks the current database version.
pub const MIGRATION_VERSION_KEY: &[u8] = b"db-version";
