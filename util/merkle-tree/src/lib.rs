// an implement for merkle tree
//          *
//        /   \
//       *     *
//      / \   / \
//     *   0  1  2
//    / \
//   3   4

pub mod merkle_tree;

use hash::Sha3;
use numext_fixed_hash::H256;

fn merge(left: &H256, right: &H256) -> H256 {
    let mut hash = [0u8; 32];
    let mut sha3 = Sha3::new_sha3_256();
    sha3.update(left.as_bytes());
    sha3.update(right.as_bytes());
    sha3.finalize(&mut hash);
    hash.into()
}

pub use self::merkle_tree::merkle_root;
