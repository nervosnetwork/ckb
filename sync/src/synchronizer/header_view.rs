use bigint::{H256, U256};
use core::header::{BlockNumber, Header};
use std::cmp::Ordering;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeaderView {
    inner: Header,
    total_difficulty: U256,
}

impl HeaderView {
    pub fn new(inner: Header, total_difficulty: U256) -> Self {
        HeaderView {
            inner,
            total_difficulty,
        }
    }

    pub fn number(&self) -> BlockNumber {
        self.inner.number()
    }

    pub fn hash(&self) -> H256 {
        self.inner.hash()
    }

    pub fn total_difficulty(&self) -> U256 {
        self.total_difficulty
    }

    pub fn inner(&self) -> &Header {
        &self.inner
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
