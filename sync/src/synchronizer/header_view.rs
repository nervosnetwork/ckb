use bigint::{H256, U256};
use core::header::IndexedHeader;
use std::cmp::Ordering;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeaderView {
    pub header: IndexedHeader,
    pub total_difficulty: U256,
}

impl HeaderView {
    pub fn new(header: IndexedHeader, total_difficulty: U256) -> Self {
        HeaderView {
            header,
            total_difficulty,
        }
    }

    pub fn hash(&self) -> H256 {
        self.header.hash()
    }
}

impl Ord for HeaderView {
    fn cmp(&self, other: &HeaderView) -> Ordering {
        self.total_difficulty.cmp(&other.total_difficulty)
    }
}

impl PartialOrd for HeaderView {
    fn partial_cmp(&self, other: &HeaderView) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
