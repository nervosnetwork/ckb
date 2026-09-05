//! Key codecs for RocksDB column families.

use ckb_gen_types::{packed, prelude::*};

/// Serialized block hash size in bytes.
pub const BLOCK_HASH_SIZE: usize = 32;
/// Block number key size: 8 bytes.
pub const BLOCK_NUMBER_KEY_SIZE: usize = 8;
/// Block key size: 8-byte block number + 32-byte block hash.
pub const BLOCK_KEY_SIZE: usize = BLOCK_NUMBER_KEY_SIZE + BLOCK_HASH_SIZE;
/// Block body key size: block key + 4-byte transaction index.
pub const BLOCK_BODY_KEY_SIZE: usize = BLOCK_KEY_SIZE + 4;
/// Cell key size: 32-byte transaction hash + 4-byte output index.
pub const CELL_KEY_SIZE: usize = BLOCK_HASH_SIZE + 4;

/// Stack-allocated block-number key.
pub type BlockNumberKey = [u8; BLOCK_NUMBER_KEY_SIZE];
/// Stack-allocated block key.
pub type BlockKey = [u8; BLOCK_KEY_SIZE];
/// Stack-allocated block body key.
pub type BlockBodyKey = [u8; BLOCK_BODY_KEY_SIZE];
/// Stack-allocated cell key.
pub type CellKey = [u8; CELL_KEY_SIZE];
/// Block hash type.
pub type BlockHash = packed::Byte32;
/// Transaction hash type.
pub type TxHash = packed::Byte32;
/// Cell out point type.
pub type OutPoint = packed::OutPoint;

/// Creates a key from a block number.
///
/// Format: `Uint64(block_number, big-endian)`.
pub fn block_number_key(number: u64) -> BlockNumberKey {
    number.to_be_bytes()
}

/// Extracts a block number from a block-number key.
pub fn decode_block_number_key(key: &[u8]) -> Option<u64> {
    Some(u64::from_be_bytes(key.try_into().ok()?))
}

/// Creates a block key.
///
/// Format: `Uint64(block_number, big-endian) + Byte32(block_hash)`.
pub fn block_key(number: u64, block_hash: &BlockHash) -> BlockKey {
    let mut key = [0u8; BLOCK_KEY_SIZE];
    key[0..BLOCK_NUMBER_KEY_SIZE].copy_from_slice(&number.to_be_bytes());
    key[BLOCK_NUMBER_KEY_SIZE..BLOCK_KEY_SIZE].copy_from_slice(block_hash.as_slice());
    key
}

/// Creates a block body key.
///
/// Format: `Uint64(block_number, big-endian) + Byte32(block_hash) + Uint32(tx_index, big-endian)`.
pub fn block_body_key(number: u64, block_hash: &BlockHash, index: u32) -> BlockBodyKey {
    let mut key = [0u8; BLOCK_BODY_KEY_SIZE];
    key[0..BLOCK_NUMBER_KEY_SIZE].copy_from_slice(&number.to_be_bytes());
    key[BLOCK_NUMBER_KEY_SIZE..BLOCK_KEY_SIZE].copy_from_slice(block_hash.as_slice());
    key[BLOCK_KEY_SIZE..BLOCK_BODY_KEY_SIZE].copy_from_slice(&index.to_be_bytes());
    key
}

/// Creates a cell key.
///
/// Format: `Byte32(tx_hash) + Uint32(output_index, big-endian)`.
pub fn cell_key(tx_hash: &TxHash, index: u32) -> CellKey {
    let mut key = [0u8; CELL_KEY_SIZE];
    key[0..BLOCK_HASH_SIZE].copy_from_slice(tx_hash.as_slice());
    key[BLOCK_HASH_SIZE..CELL_KEY_SIZE].copy_from_slice(&index.to_be_bytes());
    key
}

/// Creates a cell key from an out point.
///
/// Format: `Byte32(tx_hash) + Uint32(output_index, big-endian)`.
pub fn out_point_key(out_point: &OutPoint) -> CellKey {
    cell_key(&out_point.tx_hash(), out_point.index().unpack())
}

/// Decodes a block key into block number and block hash.
pub fn decode_block_key(key: &[u8]) -> Option<(u64, BlockHash)> {
    let key: &BlockKey = key.try_into().ok()?;
    let number = u64::from_be_bytes(key[0..BLOCK_NUMBER_KEY_SIZE].try_into().ok()?);
    let block_hash =
        packed::Byte32::new(key[BLOCK_NUMBER_KEY_SIZE..BLOCK_KEY_SIZE].try_into().ok()?);
    Some((number, block_hash))
}
