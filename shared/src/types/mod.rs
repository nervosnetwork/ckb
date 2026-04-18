#![allow(missing_docs)]
use ckb_types::core::BlockNumber;
use ckb_types::packed::Byte32;
use ckb_types::{BlockNumberAndHash, U256};

pub mod header_map;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderIndex {
    number: BlockNumber,
    hash: Byte32,
    total_difficulty: U256,
}

impl HeaderIndex {
    pub fn new(number: BlockNumber, hash: Byte32, total_difficulty: U256) -> Self {
        HeaderIndex {
            number,
            hash,
            total_difficulty,
        }
    }

    pub fn number(&self) -> BlockNumber {
        self.number
    }

    pub fn hash(&self) -> Byte32 {
        self.hash.clone()
    }

    pub fn total_difficulty(&self) -> &U256 {
        &self.total_difficulty
    }

    pub fn number_and_hash(&self) -> BlockNumberAndHash {
        (self.number(), self.hash()).into()
    }

    pub fn is_better_chain(&self, other: &Self) -> bool {
        self.is_better_than(other.total_difficulty())
    }

    pub fn is_better_than(&self, other_total_difficulty: &U256) -> bool {
        self.total_difficulty() > other_total_difficulty
    }
}

pub const SHRINK_THRESHOLD: usize = 300;
