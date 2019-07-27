use ckb_hash::new_blake2b;
use merkle_cbt::merkle_tree::Merge;
use merkle_cbt::MerkleProof as ExMerkleProof;
use merkle_cbt::MerkleTree as ExMerkleTree;
use merkle_cbt::CBMT as ExCBMT;
use numext_fixed_hash::H256;

pub struct MergeH256;

impl Merge for MergeH256 {
    type Item = H256;
    fn merge(left: &Self::Item, right: &Self::Item) -> Self::Item {
        let mut ret = [0u8; 32];
        let mut blake2b = new_blake2b();

        blake2b.update(left.as_bytes());
        blake2b.update(right.as_bytes());
        blake2b.finalize(&mut ret);
        ret.into()
    }
}

pub type MerkleProof = ExMerkleProof<H256, MergeH256>;
pub type MerkleTree = ExMerkleTree<H256, MergeH256>;
pub type CBMT = ExCBMT<H256, MergeH256>;

pub fn merkle_root(leaves: &[H256]) -> H256 {
    CBMT::build_merkle_root(leaves)
}

pub fn build_merkle_tree(leaves: Vec<H256>) -> MerkleTree {
    CBMT::build_merkle_tree(leaves)
}

pub fn build_merkle_proof(leaves: &[H256], indices: &[usize]) -> Option<MerkleProof> {
    CBMT::build_merkle_proof(leaves, indices)
}
