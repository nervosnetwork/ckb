use crate::relayer::compact_block::CompactBlock;
use crate::relayer::error::{Error, Misbehavior};
use ckb_core::transaction::{ProposalShortId, Transaction};

pub struct BlockTransactionsVerifier {}

impl BlockTransactionsVerifier {
    pub(crate) fn verify(
        block: &CompactBlock,
        indexes: &[u32],
        transactions: &[Transaction],
    ) -> Result<(), Error> {
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
            return Err(Error::Misbehavior(
                Misbehavior::InvalidBlockTransactionsLength {
                    expect: missing_short_ids.len(),
                    got: transactions.len(),
                },
            ));
        }

        for (expected_short_id, tx) in missing_short_ids.iter().zip(transactions) {
            let short_id = tx.proposal_short_id();
            if *expected_short_id != &short_id {
                return Err(Error::Misbehavior(Misbehavior::InvalidBlockTransactions {
                    expect: **expected_short_id,
                    got: short_id,
                }));
            }
        }

        Ok(())
    }
}
