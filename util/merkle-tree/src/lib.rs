use merkle_tree::{MerkleProof, MerkleTree, CBMT, H256};

pub fn merkle_root(leaves: &[H256]) -> H256 {
    CBMT::build_merkle_root(leaves)
}

pub fn build_merkle_tree(leaves: Vec<H256>) -> MerkleTree<H256> {
    CBMT::build_merkle_tree(leaves)
}

pub fn build_merkle_proof(leaves: &[H256], indices: &[usize]) -> Option<MerkleProof<H256>> {
    CBMT::build_merkle_proof(leaves, indices)
}
