use bigint::{H256, U256};
use core::header::Header;
use std::cmp::Ordering;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeaderView {
    pub header: Header,
    pub total_difficulty: U256,
}

impl HeaderView {
    pub fn new(header: Header, total_difficulty: U256) -> Self {
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
