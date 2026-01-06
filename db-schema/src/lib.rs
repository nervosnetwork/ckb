//! The schema include constants define the low level database column families.

/// Column families alias type
pub type Col = &'static str;
/// Total column number
pub const COLUMNS: u32 = 19;

/// Column store chain index (bidirectional mapping)
///
/// Key format:
/// - `Uint64` (block_number) -> Value: `Byte32` (block_hash) [main chain only]
/// - `Byte32` (block_hash) -> Value: `Uint64` (block_number) [ALL blocks]
///
/// Note: hash->number mapping exists for ALL blocks to support composite key lookup
pub const COLUMN_INDEX: Col = "0";

/// Column store block's header
///
/// Key format: `BlockKey` = `Uint64` (block_number) + `Byte32` (block_hash)
/// Value format: `HeaderView` (header data + hash)
///
/// Note: Composite key provides sequential storage by number while supporting forks
pub const COLUMN_BLOCK_HEADER: Col = "1";

/// Column store block's body (transactions)
///
/// Key format: `TransactionKey` = `Uint64` (block_number) + `Byte32` (block_hash) + `Uint32` (tx_index)
/// Value format: `TransactionView` (transaction data + hashes)
///
/// Note: Composite key provides sequential storage by number while supporting forks
pub const COLUMN_BLOCK_BODY: Col = "2";

/// Column store block's uncle and uncles' proposal zones
///
/// Key format: `BlockKey` = `Uint64` (block_number) + `Byte32` (block_hash)
/// Value format: `UncleBlockVecView` (uncle blocks data + hashes)
///
/// Note: Composite key provides sequential storage by number while supporting forks
pub const COLUMN_BLOCK_UNCLE: Col = "3";

/// Column store meta data
///
/// Key format: Various meta keys (see META_* constants below)
/// Value format: Depends on the key
/// - `META_TIP_HEADER_KEY` -> `Byte32` (tip block hash)
/// - `META_CURRENT_EPOCH_KEY` -> `EpochExt` (current epoch data)
/// - `META_LATEST_BUILT_FILTER_DATA_KEY` -> `Byte32` (block hash)
pub const COLUMN_META: Col = "4";

/// Column store transaction extra information
///
/// Key format: `Byte32` (tx_hash)
/// Value format: `TransactionInfo` (block_hash, index, block_number, block_epoch)
///
/// Note: Only stores transactions confirmed in main chain
pub const COLUMN_TRANSACTION_INFO: Col = "5";

/// Column store block extra information
///
/// Key format: `BlockKey` = `Uint64` (block_number) + `Byte32` (block_hash)
/// Value format: `BlockExt` or `BlockExtV1` (received_at, total_difficulty, verified, etc.)
///
/// Note: Composite key provides sequential storage by number while supporting forks
pub const COLUMN_BLOCK_EXT: Col = "6";

/// Column store block's proposal ids
///
/// Key format: `BlockKey` = `Uint64` (block_number) + `Byte32` (block_hash)
/// Value format: `ProposalShortIdVec` (list of proposal short ids)
///
/// Note: Composite key provides sequential storage by number while supporting forks
pub const COLUMN_BLOCK_PROPOSAL_IDS: Col = "7";

/// Column store block to epoch index mapping
///
/// Key format: `BlockKey` = `Uint64` (block_number) + `Byte32` (block_hash)
/// Value format: `Byte32` (epoch_hash/index)
///
/// Note: Composite key provides sequential storage by number while supporting forks
pub const COLUMN_BLOCK_EPOCH: Col = "8";

/// Column store epoch data (bidirectional mapping)
///
/// Key format:
/// - `Uint64` (epoch_number) -> Value: `Byte32` (epoch_hash/index)
/// - `Byte32` (epoch_hash/index) -> Value: `EpochExt` (epoch data)
///
/// Note: epoch_number provides sequential access
pub const COLUMN_EPOCH: Col = "9";

/// Column store cell (UTXO)
///
/// Key format: `OutPoint` = `Byte32` (tx_hash) + `Uint32` (index, big-endian)
/// Value format: `CellEntry` (output, block_hash, block_number, block_epoch, etc.)
///
/// Note: Uses tx_hash prefix to enable sequential traversal of outputs from same transaction
pub const COLUMN_CELL: Col = "10";

/// Column store main chain consensus include uncles
///
/// Key format: `Byte32` (uncle_hash)
/// Value format: `HeaderView` (uncle header data)
///
/// <https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0020-ckb-consensus-protocol/0020-ckb-consensus-protocol.md#specification>
pub const COLUMN_UNCLES: Col = "11";

/// Column store cell data
///
/// Key format: `OutPoint` = `Byte32` (tx_hash) + `Uint32` (index, big-endian)
/// Value format: `CellDataEntry` (output_data + output_data_hash) or empty
pub const COLUMN_CELL_DATA: Col = "12";

/// Column store block number-hash pair with transaction count
///
/// DEPRECATED: This column is no longer needed with composite keys.
/// The composite key (number + hash) is now used directly in block columns.
/// This column will be removed in a future version.
///
/// Key format: `NumberHash` = `Uint64` (block_number) + `Byte32` (block_hash)
/// Value format: `Uint32` (transaction count)
pub const COLUMN_NUMBER_HASH: Col = "13";

/// Column store cell data hash
///
/// Key format: `OutPoint` = `Byte32` (tx_hash) + `Uint32` (index, big-endian)
/// Value format: `Byte32` (data_hash) or empty
pub const COLUMN_CELL_DATA_HASH: Col = "14";

/// Column store block extension data
///
/// Key format: `BlockKey` = `Uint64` (block_number) + `Byte32` (block_hash)
/// Value format: `Bytes` (extension data)
///
/// Note: Composite key provides sequential storage by number while supporting forks
pub const COLUMN_BLOCK_EXTENSION: Col = "15";

/// Column store chain root MMR data
///
/// Key format: `Uint64` (position)
/// Value format: `HeaderDigest` (MMR digest data)
///
/// Note: Uses sequential position as key, good for performance
pub const COLUMN_CHAIN_ROOT_MMR: Col = "16";

/// Column store filter data for client-side filtering
///
/// Key format: `BlockKey` = `Uint64` (block_number) + `Byte32` (block_hash)
/// Value format: `Bytes` (filter data)
///
/// Note: Composite key provides sequential storage by number while supporting forks
pub const COLUMN_BLOCK_FILTER: Col = "17";

/// Column store filter data hash for client-side filtering
///
/// Key format: `BlockKey` = `Uint64` (block_number) + `Byte32` (block_hash)
/// Value format: `Byte32` (filter_hash)
///
/// Note: Composite key provides sequential storage by number while supporting forks
pub const COLUMN_BLOCK_FILTER_HASH: Col = "18";

/// META_TIP_HEADER_KEY tracks the latest known best block header
pub const META_TIP_HEADER_KEY: &[u8] = b"TIP_HEADER";
/// META_CURRENT_EPOCH_KEY tracks the latest known epoch
pub const META_CURRENT_EPOCH_KEY: &[u8] = b"CURRENT_EPOCH";
/// META_FILTER_DATA_KEY tracks the latest built filter data block hash
pub const META_LATEST_BUILT_FILTER_DATA_KEY: &[u8] = b"LATEST_BUILT_FILTER_DATA";

/// CHAIN_SPEC_HASH_KEY tracks the hash of chain spec which created current database
pub const CHAIN_SPEC_HASH_KEY: &[u8] = b"chain-spec-hash";
/// MIGRATION_VERSION_KEY tracks the current database version.
pub const MIGRATION_VERSION_KEY: &[u8] = b"db-version";
