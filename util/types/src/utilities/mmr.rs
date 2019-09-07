use crate::{packed::HeaderDigest, prelude::*, U256};
use ckb_hash::new_blake2b;
use ckb_merkle_mountain_range::Merge;

pub struct MergeHeaderDigest;

impl Merge for MergeHeaderDigest {
    type Item = HeaderDigest;
    fn merge(lhs: &Self::Item, rhs: &Self::Item) -> Self::Item {
        let mut hasher = new_blake2b();
        let mut hash = [0u8; 32];
        let lhs_hash: [u8; 32] = lhs.hash().unpack();
        let rhs_hash: [u8; 32] = lhs.hash().unpack();
        hasher.update(&lhs_hash);
        hasher.update(&rhs_hash);
        hasher.finalize(&mut hash);
        let lhs_td: U256 = lhs.total_difficulty().unpack();
        let rhs_td: U256 = rhs.total_difficulty().unpack();
        let total_difficulty = lhs_td + rhs_td;
        HeaderDigest::new_builder()
            .hash(hash.pack())
            .total_difficulty(total_difficulty.pack())
            .build()
    }
}
