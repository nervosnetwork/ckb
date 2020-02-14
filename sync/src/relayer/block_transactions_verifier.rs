use crate::{Status, StatusCode};
use ckb_types::{core, packed};

pub struct BlockTransactionsVerifier {}

impl BlockTransactionsVerifier {
    pub(crate) fn verify(
        block: &packed::CompactBlock,
        indexes: &[u32],
        transactions: &[core::TransactionView],
    ) -> Status {
        let block_short_ids = block.block_short_ids();
        let missing_short_ids: Vec<packed::ProposalShortId> = indexes
            .iter()
            .filter_map(|index| {
                block_short_ids
                    .get(*index as usize)
                    .expect("should never outbound")
                    .clone()
            })
            .collect();

        if missing_short_ids.len() != transactions.len() {
            return StatusCode::BlockTransactionsLengthIsUnmatchedWithPendingCompactBlock
                .with_context(format!(
                    "Expected({}) != actual({})",
                    missing_short_ids.len(),
                    transactions.len(),
                ));
        }

        for (expected_short_id, tx) in missing_short_ids.into_iter().zip(transactions) {
            let short_id = tx.proposal_short_id();
            if expected_short_id != short_id {
                return StatusCode::BlockTransactionsShortIdsAreUnmatchedWithPendingCompactBlock
                    .with_context(format!(
                        "Expected({}) != actual({})",
                        expected_short_id, short_id,
                    ));
            }
        }

        Status::ok()
    }
}
