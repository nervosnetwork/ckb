use super::helper::{build_chain, new_transaction};
use crate::relayer::compact_block::{CompactBlock, ShortTransactionID};
use ckb_core::transaction::{IndexTransaction, Transaction};
use ckb_protocol::{short_transaction_id, short_transaction_id_keys};

#[test]
fn test_reconstruct_block() {
    let (relayer, always_success_out_point) = build_chain(5);
    let prepare: Vec<Transaction> = (0..20)
        .map(|i| new_transaction(&relayer, i, &always_success_out_point))
        .collect();

    // Case: miss tx.0
    {
        let mut compact = CompactBlock {
            nonce: 2,
            ..Default::default()
        };
        let (key0, key1) = short_transaction_id_keys(compact.header.nonce(), compact.nonce);
        let short_ids = prepare
            .iter()
            .map(|tx| short_transaction_id(key0, key1, &tx.witness_hash()))
            .collect();
        let transactions: Vec<Transaction> = prepare.iter().skip(1).cloned().collect();
        compact.short_ids = short_ids;
        assert_eq!(
            relayer.reconstruct_block(&compact, transactions),
            Err(vec![0]),
        );
    }

    // Case: miss multiple txs
    {
        let mut compact = CompactBlock {
            nonce: 2,
            ..Default::default()
        };
        let (key0, key1) = short_transaction_id_keys(compact.header.nonce(), compact.nonce);
        let short_ids = prepare
            .iter()
            .map(|tx| short_transaction_id(key0, key1, &tx.witness_hash()))
            .collect();
        let transactions: Vec<Transaction> = prepare.iter().skip(1).step_by(2).cloned().collect();
        let missing = prepare
            .iter()
            .enumerate()
            .step_by(2)
            .map(|(i, _)| i)
            .collect();
        compact.short_ids = short_ids;
        assert_eq!(
            relayer.reconstruct_block(&compact, transactions),
            Err(missing),
        );
    }

    // Case: short transactions lie on pool but not proposed, cannot be used to reconstruct block
    {
        let mut compact = CompactBlock {
            nonce: 3,
            ..Default::default()
        };
        let (key0, key1) = short_transaction_id_keys(compact.header.nonce(), compact.nonce);
        let (short_transactions, prefilled) = {
            let short_transactions: Vec<Transaction> = prepare.iter().step_by(2).cloned().collect();
            let prefilled: Vec<IndexTransaction> = prepare
                .iter()
                .enumerate()
                .skip(1)
                .step_by(2)
                .map(|(i, tx)| IndexTransaction {
                    index: i,
                    transaction: tx.clone(),
                })
                .collect();
            (short_transactions, prefilled)
        };
        let short_ids: Vec<ShortTransactionID> = short_transactions
            .iter()
            .map(|tx| short_transaction_id(key0, key1, &tx.witness_hash()))
            .collect();
        compact.short_ids = short_ids;
        compact.prefilled_transactions = prefilled;

        // Split first 2 short transactions and move into pool. These pool transactions are not
        // proposed, so it will not be acquired inside `reconstruct_block`
        let (pool_transactions, short_transactions) = short_transactions.split_at(2);
        let short_transactions: Vec<Transaction> = short_transactions.to_vec();
        pool_transactions.iter().for_each(|tx| {
            // `tx` is added into pool but not be proposed, since `tx` has not been proposal yet
            relayer
                .tx_pool_executor
                .verify_and_add_tx_to_pool(tx.clone())
                .expect("adding transaction into pool");
        });

        assert_eq!(
            relayer.reconstruct_block(&compact, short_transactions),
            Err(vec![0, 2]),
        );
    }
}
