#![allow(missing_docs)]
use ckb_types::core::{BlockNumber, EpochNumberWithFraction};
use ckb_types::packed::Byte32;
use ckb_types::{BlockNumberAndHash, U256};

pub mod header_map;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderIndexView {
    hash: Byte32,
    number: BlockNumber,
    epoch: EpochNumberWithFraction,
    timestamp: u64,
    parent_hash: Byte32,
    total_difficulty: U256,
}

impl HeaderIndexView {
    pub fn new(
        hash: Byte32,
        number: BlockNumber,
        epoch: EpochNumberWithFraction,
        timestamp: u64,
        parent_hash: Byte32,
        total_difficulty: U256,
    ) -> Self {
        HeaderIndexView {
            hash,
            number,
            epoch,
            timestamp,
            parent_hash,
            total_difficulty,
        }
    }

    pub fn hash(&self) -> Byte32 {
        self.hash.clone()
    }

    pub fn number(&self) -> BlockNumber {
        self.number
    }

    pub fn epoch(&self) -> EpochNumberWithFraction {
        self.epoch
    }

    pub fn timestamp(&self) -> u64 {
        self.timestamp
    }

    pub fn total_difficulty(&self) -> &U256 {
        &self.total_difficulty
    }

    pub fn parent_hash(&self) -> Byte32 {
        self.parent_hash.clone()
    }

    pub fn get_ancestor<F, G>(
        &self,
        tip_number: BlockNumber,
        number: BlockNumber,
        get_header_view: F,
        fast_scanner: G,
    ) -> Option<HeaderIndexView>
    where
        F: Fn(&Byte32, bool) -> Option<HeaderIndexView>,
        G: Fn(BlockNumber, BlockNumberAndHash) -> Option<HeaderIndexView>,
    {
        if number > self.number() {
            return None;
        }

        let mut current = self.clone();
        while current.number() > number {
            // Try fast scanner optimization first
            if let Some(target) = fast_scanner(number, (current.number(), current.hash()).into()) {
                current = target;
                break;
            }

            // Fall back to parent traversal
            let store_first = current.number() <= tip_number;
            current = get_header_view(&current.parent_hash(), store_first)?;
        }
        Some(current)
    }

    pub fn as_header_index(&self) -> HeaderIndex {
        HeaderIndex::new(self.number(), self.hash(), self.total_difficulty().clone())
    }

    pub fn number_and_hash(&self) -> BlockNumberAndHash {
        (self.number(), self.hash()).into()
    }

    pub fn is_better_than(&self, total_difficulty: &U256) -> bool {
        self.total_difficulty() > total_difficulty
    }
}

impl From<(ckb_types::core::HeaderView, U256)> for HeaderIndexView {
    fn from((header, total_difficulty): (ckb_types::core::HeaderView, U256)) -> Self {
        HeaderIndexView {
            hash: header.hash(),
            number: header.number(),
            epoch: header.epoch(),
            timestamp: header.timestamp(),
            parent_hash: header.parent_hash(),
            total_difficulty,
        }
    }
}

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
