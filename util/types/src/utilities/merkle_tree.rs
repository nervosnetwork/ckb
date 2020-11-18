use ckb_hash::new_blake2b;
use merkle_cbt::{merkle_tree::Merge, MerkleProof as ExMerkleProof, CBMT as ExCBMT};

use crate::{packed::Byte32, prelude::*};

/// TODO(doc): @quake
pub struct MergeByte32;

impl Merge for MergeByte32 {
    type Item = Byte32;
    fn merge(left: &Self::Item, right: &Self::Item) -> Self::Item {
        let mut ret = [0u8; 32];
        let mut blake2b = new_blake2b();

        blake2b.update(left.as_slice());
        blake2b.update(right.as_slice());
        blake2b.finalize(&mut ret);
        ret.pack()
    }
}

/// TODO(doc): @quake
pub type CBMT = ExCBMT<Byte32, MergeByte32>;
/// TODO(doc): @quake
pub type MerkleProof = ExMerkleProof<Byte32, MergeByte32>;

/// TODO(doc): @quake
pub fn merkle_root(leaves: &[Byte32]) -> Byte32 {
    CBMT::build_merkle_root(leaves)
}
