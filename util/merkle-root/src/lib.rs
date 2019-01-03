use hash::Sha3;
use merkle_tree::{HashKernels, Tree};
use numext_fixed_hash::H256;

pub struct H256Sha3;

impl HashKernels for H256Sha3 {
    type Item = H256;

    fn merge(left: &Self::Item, right: &Self::Item) -> Self::Item {
        let mut hash = [0u8; 32];
        let mut sha3 = Sha3::new_sha3_256();
        sha3.update(left.as_bytes());
        sha3.update(right.as_bytes());
        sha3.finalize(&mut hash);
        hash.into()
    }
}

pub fn merkle_root(input: &[H256]) -> H256 {
    Tree::<H256Sha3>::build_root(input).unwrap_or_else(H256::zero)
}
