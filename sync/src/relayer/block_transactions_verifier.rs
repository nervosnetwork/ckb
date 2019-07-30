use crate::relayer::compact_block::CompactBlock;
use crate::relayer::error::{Error, Misbehavior};
use ckb_core::transaction::Transaction;
use ckb_protocol::{short_transaction_id, short_transaction_id_keys, ShortTransactionID};

pub struct BlockTransactionsVerifier {}

impl BlockTransactionsVerifier {
    pub(crate) fn verify(
        block: &CompactBlock,
        indexes: &[u32],
        transactions: &[Transaction],
    ) -> Result<(), Error> {
        let block_short_ids = block.block_short_ids();
        let missing_short_ids: Vec<&ShortTransactionID> = indexes
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

        let (key0, key1) = short_transaction_id_keys(block.header.nonce(), block.nonce);

        for (expected_short_id, tx) in missing_short_ids.iter().zip(transactions) {
            let short_id = short_transaction_id(key0, key1, &tx.witness_hash());
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
