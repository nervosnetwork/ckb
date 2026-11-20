//! V1 RocksDB column family names.

use crate::Col;

/// Total current column family number.
pub const COLUMNS: u32 = 19;

/// Current column family names in creation/open order.
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
    COLUMN_CELL_DATA_HASH,
    COLUMN_BLOCK_EXTENSION,
    COLUMN_CHAIN_ROOT_MMR,
    COLUMN_BLOCK_FILTER,
    COLUMN_BLOCK_FILTER_HASH,
    COLUMN_HASH_INDEX,
];

const _: [(); COLUMNS as usize] = [(); COLUMN_FAMILIES.len()];

/// Column store main chain block number to block hash mapping
///
/// Key formats:
/// - `Uint64` (block_number, big-endian) -> Value: `Byte32` (block_hash) [main chain only]
///
/// Operations:
/// - attach_block(): Insert number->hash
/// - detach_block(): Delete number->hash
/// - get_block_hash(): Read number->hash
pub const COLUMN_INDEX: Col = "index";

/// Column store block hash to number mapping with is_main_chain flag
///
/// Key format: `Byte32` (block_hash)
/// Value format: 9 bytes = 8 bytes (number, big-endian) + 1 byte (0x01 if main chain, 0x00 if fork)
///
/// The hash->number mapping stores ALL blocks (main chain + forks) with a flag byte:
/// - This enables both composite key lookup and O(1) is_main_chain check in a single DB operation
///
/// Operations:
/// - insert_block(): Insert hash->(number, 0x00)
/// - attach_block(): Update hash->(number, 0x01)
/// - detach_block(): Update hash->(number, 0x00)
/// - delete_block(): Delete hash->(number, flag)
/// - is_main_chain(): Read hash->value, check flag == 0x01
/// - get_block_key(): Read hash->value, extract number, build composite key
pub const COLUMN_HASH_INDEX: Col = "hash_index";

/// Column store block's header
///
/// Key format: `BlockKey` = `Uint64` (block_number, big-endian) + `Byte32` (block_hash)
/// Value format: `HeaderView` (header data + hash)
///
/// Note: Composite key provides sequential storage by number while supporting forks
pub const COLUMN_BLOCK_HEADER: Col = "block_header";

/// Column store block's body (transactions)
///
/// Key format: `TransactionKey` = `Uint64` (block_number, big-endian) + `Byte32` (block_hash) + `Uint32` (tx_index)
/// Value format: `TransactionView` (transaction data + hashes)
///
/// Note: Composite key provides sequential storage by number while supporting forks
pub const COLUMN_BLOCK_BODY: Col = "block_body";

/// Column store block's uncle and uncles' proposal zones
///
/// Key format: `BlockKey` = `Uint64` (block_number, big-endian) + `Byte32` (block_hash)
/// Value format: `UncleBlockVecView` (uncle blocks data + hashes)
///
/// Note: Composite key provides sequential storage by number while supporting forks
pub const COLUMN_BLOCK_UNCLE: Col = "block_uncle";

/// Column store meta data
///
/// Key format: Various meta keys (see META_* constants below)
/// Value format: Depends on the key
/// - `META_TIP_HEADER_KEY` -> `Byte32` (tip block hash)
/// - `META_CURRENT_EPOCH_KEY` -> `EpochExt` (current epoch data)
/// - `META_LATEST_BUILT_FILTER_DATA_KEY` -> `Byte32` (block hash)
pub const COLUMN_META: Col = "meta";

/// Column store transaction extra information
///
/// Key format: `Byte32` (tx_hash)
/// Value format: `TransactionInfo` (block_hash, index, block_number, block_epoch)
///
/// Note: Only stores transactions confirmed in main chain
pub const COLUMN_TRANSACTION_INFO: Col = "transaction_info";

/// Column store block extra information
///
/// Key format: `BlockKey` = `Uint64` (block_number, big-endian) + `Byte32` (block_hash)
/// Value format: `BlockExt` or `BlockExtV1` (received_at, total_difficulty, verified, etc.)
///
/// Note: Composite key provides sequential storage by number while supporting forks
pub const COLUMN_BLOCK_EXT: Col = "block_ext";

/// Column store block's proposal ids
///
/// Key format: `BlockKey` = `Uint64` (block_number, big-endian) + `Byte32` (block_hash)
/// Value format: `ProposalShortIdVec` (list of proposal short ids)
///
/// Note: Composite key provides sequential storage by number while supporting forks
pub const COLUMN_BLOCK_PROPOSAL_IDS: Col = "block_proposal_ids";

/// Column store block to epoch index mapping
///
/// Key format: `BlockKey` = `Uint64` (block_number, big-endian) + `Byte32` (block_hash)
/// Value format: `Byte32` (epoch_hash/index)
///
/// Note: Composite key provides sequential storage by number while supporting forks
pub const COLUMN_BLOCK_EPOCH: Col = "block_epoch";

/// Column store epoch data (bidirectional mapping)
///
/// Key format:
/// - `Uint64` (epoch_number) -> Value: `Byte32` (epoch_hash/index)
/// - `Byte32` (epoch_hash/index) -> Value: `EpochExt` (epoch data)
///
/// Note: epoch_number provides sequential access
pub const COLUMN_EPOCH: Col = "epoch";

/// Column store cell (UTXO)
///
/// Key format: `OutPoint` = `Byte32` (tx_hash) + `Uint32` (index, big-endian)
/// Value format: `CellEntry` (output, block_hash, block_number, block_epoch, etc.)
///
/// Note: Uses tx_hash prefix to enable sequential traversal of outputs from same transaction
pub const COLUMN_CELL: Col = "cell";

/// Column store main chain consensus include uncles
///
/// Key format: `Byte32` (uncle_hash)
/// Value format: `HeaderView` (uncle header data)
///
/// <https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0020-ckb-consensus-protocol/0020-ckb-consensus-protocol.md#specification>
pub const COLUMN_UNCLES: Col = "uncles";

/// Column store cell data
///
/// Key format: `OutPoint` = `Byte32` (tx_hash) + `Uint32` (index, big-endian)
/// Value format: `CellDataEntry` (output_data + output_data_hash) or empty
pub const COLUMN_CELL_DATA: Col = "cell_data";

/// Column store cell data hash
///
/// Key format: `OutPoint` = `Byte32` (tx_hash) + `Uint32` (index, big-endian)
/// Value format: `Byte32` (data_hash) or empty
pub const COLUMN_CELL_DATA_HASH: Col = "cell_data_hash";

/// Column store block extension data
///
/// Key format: `BlockKey` = `Uint64` (block_number, big-endian) + `Byte32` (block_hash)
/// Value format: `Bytes` (extension data)
///
/// Note: Composite key provides sequential storage by number while supporting forks
pub const COLUMN_BLOCK_EXTENSION: Col = "block_extension";

/// Column store chain root MMR data
///
/// Key format: `Uint64` (position)
/// Value format: `HeaderDigest` (MMR digest data)
///
/// Note: Uses sequential position as key, good for performance
pub const COLUMN_CHAIN_ROOT_MMR: Col = "chain_root_mmr";

/// Column store filter data for client-side filtering
///
/// Key format: `BlockKey` = `Uint64` (block_number, big-endian) + `Byte32` (block_hash)
/// Value format: `Bytes` (filter data)
///
/// Note: Composite key provides sequential storage by number while supporting forks
pub const COLUMN_BLOCK_FILTER: Col = "block_filter";

/// Column store filter data hash for client-side filtering
///
/// Key format: `BlockKey` = `Uint64` (block_number, big-endian) + `Byte32` (block_hash)
/// Value format: `Byte32` (filter_hash)
///
/// Note: Composite key provides sequential storage by number while supporting forks
pub const COLUMN_BLOCK_FILTER_HASH: Col = "block_filter_hash";
