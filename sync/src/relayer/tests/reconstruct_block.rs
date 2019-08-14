use super::helper::{build_chain, new_transaction};
use ckb_types::prelude::*;
use ckb_types::{
    core::TransactionView,
    packed::{CompactBlockBuilder, IndexTransaction, IndexTransactionBuilder},
};

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
            Err(vec![0]),
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
            Err(missing),
        );
    }

    // Case: short transactions lie on pool but not proposed, can be used to reconstruct block also
    {
        let compact_block_builder = CompactBlockBuilder::default();
        let (short_transactions, prefilled) = {
            let short_transactions: Vec<TransactionView> =
                prepare.iter().step_by(2).cloned().collect();
            let prefilled: Vec<IndexTransaction> = prepare
                .iter()
                .enumerate()
                .skip(1)
                .step_by(2)
                .map(|(i, tx)| {
                    IndexTransactionBuilder::default()
                        .index(i.pack())
                        .transaction(tx.data())
                        .build()
                })
                .collect();
            (short_transactions, prefilled)
        };
        let short_ids = short_transactions.iter().map(|tx| tx.proposal_short_id());
        let compact = compact_block_builder
            .short_ids(short_ids.pack())
            .prefilled_transactions(prefilled.into_iter().pack())
            .build();

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
