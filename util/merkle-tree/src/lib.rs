use merkle_tree::{new_cbmt, MerkleProof, MerkleTree, H256};

pub fn merkle_root(leaves: &[H256]) -> H256 {
    let cbmt = new_cbmt::<H256>();
    cbmt.build_merkle_root(leaves)
}

pub fn build_merkle_tree(leaves: &[H256]) -> MerkleTree<H256> {
    let cbmt = new_cbmt::<H256>();
    cbmt.build_merkle_tree(leaves)
}

pub fn build_merkle_proof(leaves: &[H256], indices: Vec<usize>) -> Option<MerkleProof<H256>> {
    let cbmt = new_cbmt::<H256>();
    cbmt.build_merkle_proof(leaves, indices)
}
