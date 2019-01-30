use merkle_cbt::MerkleProof as ExMerkleProof;
use merkle_cbt::MerkleTree as ExMerkleTree;
use merkle_cbt::CBMT as ExCBMT;
use merkle_cbt::H256;

pub type MerkleProof = ExMerkleProof<H256>;
pub type MerkleTree = ExMerkleTree<H256>;
pub type CBMT = ExCBMT<H256>;

pub fn merkle_root(leaves: &[H256]) -> H256 {
    CBMT::build_merkle_root(leaves)
}

pub fn build_merkle_tree(leaves: Vec<H256>) -> MerkleTree {
    CBMT::build_merkle_tree(leaves)
}

pub fn build_merkle_proof(leaves: &[H256], indices: &[usize]) -> Option<MerkleProof> {
    CBMT::build_merkle_proof(leaves, indices)
}
