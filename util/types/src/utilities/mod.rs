//! TODO(doc): @doitian
mod difficulty;
mod merkle_tree;

pub use difficulty::{
    compact_to_difficulty, compact_to_target, difficulty_to_compact, target_to_compact, DIFF_TWO,
};
pub use merkle_tree::{merkle_root, MergeByte32, MerkleProof, CBMT};
