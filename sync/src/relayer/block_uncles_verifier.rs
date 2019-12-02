use crate::{Status, StatusCode};
use ckb_types::{core, packed};

pub struct BlockUnclesVerifier {}

impl BlockUnclesVerifier {
    pub(crate) fn verify(
        block: &packed::CompactBlock,
        indexes: &[u32],
        uncles: &[core::UncleBlockView],
    ) -> Status {
        let expected_uncles = block.uncles();
        let expected_ids: Vec<packed::Byte32> = indexes
            .iter()
            .filter_map(|index| expected_uncles.get(*index as usize))
            .collect();

        if expected_ids.len() != uncles.len() {
            StatusCode::UnmatchedBlockUnclesLength.with_context(format!(
                "Expected({}) != actual({})",
                expected_ids.len(),
                uncles.len(),
            ));
        }

        for (expected_id, uncle) in expected_ids.into_iter().zip(uncles) {
            let hash = uncle.hash();
            if hash != expected_id {
                return StatusCode::UnmatchedBlockUncles
                    .with_context(format!("Expected({}) != actual({})", expected_id, hash,));
            }
        }

        Status::ok()
    }
}
