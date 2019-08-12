use crate::relayer::compact_block::CompactBlock;
use crate::{Status, StatusCode};
use ckb_core::transaction::{ProposalShortId, Transaction};

pub struct BlockTransactionsVerifier {}

impl BlockTransactionsVerifier {
    pub(crate) fn verify(
        block: &CompactBlock,
        indexes: &[u32],
        transactions: &[Transaction],
    ) -> Status {
        let block_hash = block.header.hash();
        let block_number = block.header.number();
        let block_short_ids = block.block_short_ids();
        let missing_short_ids: Vec<&ProposalShortId> = indexes
            .iter()
            .filter_map(|index| {
                block_short_ids
                    .get(*index as usize)
                    .expect("should never outbound")
                    .as_ref()
            })
            .collect();

        if missing_short_ids.len() != transactions.len() {
            return StatusCode::UnmatchedBlockTransactionsLength.with_context(format!(
                "expected: {}, actual: {}, #{} {:#x}",
                missing_short_ids.len(),
                transactions.len(),
                block_number,
                block_hash,
            ));
        }

        for (expected_short_id, tx) in missing_short_ids.iter().zip(transactions) {
            let short_id = tx.proposal_short_id();
            if *expected_short_id != &short_id {
                return StatusCode::UnmatchedBlockTransactions.with_context(format!(
                    "expected: {:?}, actual: {:?}, #{}, {:#x}",
                    expected_short_id, short_id, block_number, block_hash,
                ));
            }
        }

        Status::ok()
    }
}
