use crate::{core::views::HeaderView, packed, prelude::*, U256};
use ckb_hash::new_blake2b;
use ckb_merkle_mountain_range::{MerkleElem, Result as MMRResult};

#[derive(Clone, Debug)]
pub struct HeaderDigest {
    data: packed::HeaderDigest,
}

impl HeaderDigest {
    pub fn data(&self) -> &packed::HeaderDigest {
        &self.data
    }
}

impl From<packed::HeaderDigest> for HeaderDigest {
    fn from(data: packed::HeaderDigest) -> Self {
        HeaderDigest { data }
    }
}

impl From<HeaderView> for HeaderDigest {
    fn from(header_view: HeaderView) -> Self {
        let data = packed::HeaderDigest::new_builder()
            .hash(header_view.hash())
            .total_difficulty(header_view.difficulty().pack())
            .build();
        HeaderDigest { data }
    }
}

impl PartialEq for HeaderDigest {
    fn eq(&self, other: &Self) -> bool {
        self.data().as_slice().eq(other.data().as_slice())
    }
}

impl MerkleElem for HeaderDigest {
    fn merge(lhs: &Self, rhs: &Self) -> MMRResult<Self> {
        let mut hasher = new_blake2b();
        let mut hash = [0u8; 32];
        let lhs_hash: [u8; 32] = lhs.data.hash().unpack();
        let rhs_hash: [u8; 32] = lhs.data.hash().unpack();
        hasher.update(&lhs_hash);
        hasher.update(&rhs_hash);
        hasher.finalize(&mut hash);
        let lhs_td: U256 = lhs.data.total_difficulty().unpack();
        let rhs_td: U256 = rhs.data.total_difficulty().unpack();
        let total_difficulty = lhs_td + rhs_td;
        let data = packed::HeaderDigest::new_builder()
            .hash(hash.pack())
            .total_difficulty(total_difficulty.pack())
            .build();
        Ok(HeaderDigest { data })
    }
}
