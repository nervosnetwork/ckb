use ckb_core::header::{BlockNumber, Header};
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeaderView {
    inner: Header,
    total_difficulty: U256,
    total_uncles_count: u64,
}

impl HeaderView {
    pub fn new(inner: Header, total_difficulty: U256, total_uncles_count: u64) -> Self {
        HeaderView {
            inner,
            total_difficulty,
            total_uncles_count,
        }
    }

    pub fn number(&self) -> BlockNumber {
        self.inner.number()
    }

    pub fn hash(&self) -> &H256 {
        self.inner.hash()
    }

    pub fn total_uncles_count(&self) -> u64 {
        self.total_uncles_count
    }

    pub fn total_difficulty(&self) -> &U256 {
        &self.total_difficulty
    }

    pub fn inner(&self) -> &Header {
        &self.inner
    }

    pub fn into_inner(self) -> Header {
        self.inner
    }
}
