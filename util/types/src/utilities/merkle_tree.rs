use ckb_hash::new_blake2b;
use merkle_cbt::{CBMT as ExCBMT, MerkleProof as ExMerkleProof, merkle_tree::Merge};

use crate::{packed::Byte32, prelude::*};

/// Merge function for computing Merkle tree nodes from pairs of `Byte32` values.
pub struct MergeByte32;

impl Merge for MergeByte32 {
    type Item = Byte32;
    fn merge(left: &Self::Item, right: &Self::Item) -> Self::Item {
        let mut ret = [0u8; 32];
        let mut blake2b = new_blake2b();

        blake2b.update(left.as_slice());
        blake2b.update(right.as_slice());
        blake2b.finalize(&mut ret);
        ret.into()
    }
}

/// Complete Binary Merkle Tree specialized for `Byte32` leaves.
pub type CBMT = ExCBMT<Byte32, MergeByte32>;
/// Merkle proof for `Byte32` values.
pub type MerkleProof = ExMerkleProof<Byte32, MergeByte32>;

/// Computes the Merkle root from a list of leaves.
pub fn merkle_root(leaves: &[Byte32]) -> Byte32 {
    CBMT::build_merkle_root(leaves)
}
