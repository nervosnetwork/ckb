use super::helper::{build_chain, new_transaction};
use crate::relayer::ReconstructionError;
use ckb_types::prelude::*;
use ckb_types::{
    core::TransactionView,
    packed,
    packed::{BlockBuilder, CompactBlockBuilder},
};
use std::collections::HashSet;

#[test]
fn test_reconstruct_block() {
    let (relayer, always_success_out_point) = build_chain(5);
    let prepare: Vec<TransactionView> = (0..20)
        .map(|i| new_transaction(&relayer, i, &always_success_out_point))
        .collect();

    // Case: miss tx.0
    {
        let compact_block_builder = CompactBlockBuilder::default();
        let short_ids = prepare.iter().map(|tx| tx.proposal_short_id());
        let transactions: Vec<TransactionView> = prepare.iter().skip(1).cloned().collect();
        let compact = compact_block_builder.short_ids(short_ids.pack()).build();
        assert_eq!(
            relayer.reconstruct_block(&compact, transactions),
            Err(ReconstructionError::MissingIndexes(vec![0])),
        );
    }

    // Case: miss multiple txs
    {
        let compact_block_builder = CompactBlockBuilder::default();
        let short_ids = prepare.iter().map(|tx| tx.proposal_short_id());
        let transactions: Vec<TransactionView> =
            prepare.iter().skip(1).step_by(2).cloned().collect();
        let missing = prepare
            .iter()
            .enumerate()
            .step_by(2)
            .map(|(i, _)| i)
            .collect();
        let compact = compact_block_builder.short_ids(short_ids.pack()).build();
        assert_eq!(
            relayer.reconstruct_block(&compact, transactions),
            Err(ReconstructionError::MissingIndexes(missing)),
        );
    }

    // Case: short transactions lie on pool but not proposed, can be used to reconstruct block also
    {
        let (short_transactions, prefilled) = {
            let short_transactions: Vec<TransactionView> =
                prepare.iter().step_by(2).cloned().collect();
            let prefilled: HashSet<usize> = prepare
                .iter()
                .enumerate()
                .skip(1)
                .step_by(2)
                .map(|(i, _)| i)
                .collect();
            (short_transactions, prefilled)
        };

        let block = BlockBuilder::default()
            .transactions(prepare.into_iter().map(|v| v.data()).pack())
            .build();

        let compact = packed::CompactBlock::build_from_block(&block.into_view(), &prefilled);

        // Should reconstruct block successfully with pool txs
        let (pool_transactions, short_transactions) = short_transactions.split_at(2);
        let short_transactions: Vec<TransactionView> = short_transactions.to_vec();
        pool_transactions.iter().for_each(|tx| {
            // `tx` is added into pool but not be proposed, since `tx` has not been proposal yet
            relayer
                .tx_pool_executor
                .verify_and_add_tx_to_pool(tx.clone())
                .expect("adding transaction into pool");
        });

        assert!(relayer
            .reconstruct_block(&compact, short_transactions)
            .is_ok());
    }
}
