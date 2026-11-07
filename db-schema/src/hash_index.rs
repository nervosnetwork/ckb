//! Value codec for `COLUMN_HASH_INDEX`.

/// Flag stored in the last byte of a `COLUMN_HASH_INDEX` value.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockIndexFlag {
    /// The block is stored but is not part of the current main chain.
    Fork = 0,
    /// The block is part of the current main chain.
    MainChain = 1,
}

/// Decoded value stored in `COLUMN_HASH_INDEX`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockHashIndexValue {
    /// Block number for the indexed block hash.
    pub number: u64,
    /// Whether the indexed block is canonical or forked.
    pub flag: BlockIndexFlag,
}

impl BlockHashIndexValue {
    /// Encoded value length in bytes.
    pub const SIZE: usize = 9;

    /// Encodes as `Uint64(block_number, big-endian) + u8(BlockIndexFlag)`.
    pub fn encode(self) -> [u8; Self::SIZE] {
        let mut value = [0u8; Self::SIZE];
        value[0..8].copy_from_slice(&self.number.to_be_bytes());
        value[8] = self.flag as u8;
        value
    }

    /// Decodes `Uint64(block_number, big-endian) + u8(BlockIndexFlag)`.
    pub fn decode(value: &[u8]) -> Option<Self> {
        let number = u64::from_be_bytes(value.get(0..8)?.try_into().ok()?);
        let flag = match *value.get(8)? {
            0x00 => BlockIndexFlag::Fork,
            0x01 => BlockIndexFlag::MainChain,
            _ => return None,
        };
        Some(Self { number, flag })
    }
}
