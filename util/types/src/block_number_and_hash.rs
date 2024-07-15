//! Block Number and Hash struct
use crate::core::BlockNumber;
use ckb_gen_types::packed::Byte32;

/// Block Number And Hash struct
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockNumberAndHash {
    /// Block Number
    pub number: BlockNumber,
    /// Block Hash
    pub hash: Byte32,
}

impl BlockNumberAndHash {
    /// Create new BlockNumberAndHash
    pub fn new(number: BlockNumber, hash: Byte32) -> Self {
        Self { number, hash }
    }
    /// Return BlockNumber
    pub fn number(&self) -> BlockNumber {
        self.number
    }
    /// Return Hash
    pub fn hash(&self) -> Byte32 {
        self.hash.clone()
    }
}

impl From<(BlockNumber, Byte32)> for BlockNumberAndHash {
    fn from(inner: (BlockNumber, Byte32)) -> Self {
        Self {
            number: inner.0,
            hash: inner.1,
        }
    }
}

impl From<&crate::core::HeaderView> for BlockNumberAndHash {
    fn from(header: &crate::core::HeaderView) -> Self {
        Self {
            number: header.number(),
            hash: header.hash(),
        }
    }
}

impl From<crate::core::HeaderView> for BlockNumberAndHash {
    fn from(header: crate::core::HeaderView) -> Self {
        Self {
            number: header.number(),
            hash: header.hash(),
        }
    }
}
