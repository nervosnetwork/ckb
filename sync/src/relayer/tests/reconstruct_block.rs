use super::helper::{build_chain, new_transaction};
use crate::relayer::ReconstructionError;
use ckb_tx_pool::{PlugTarget, TxEntry};
use ckb_types::prelude::*;
use ckb_types::{
    core::{Capacity, TransactionView},
    packed,
    packed::{BlockBuilder, CompactBlockBuilder},
};
use std::collections::HashSet;

// There are more test cases in block_transactions_process and compact_block_process.rs
#[test]
fn test_missing_txs() {
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
            relayer.reconstruct_block(
                &relayer.shared().snapshot(),
                &compact,
                transactions,
                &[],
                &[]
            ),
            Err(ReconstructionError::MissingIndexes(vec![0], vec![])),
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
            relayer.reconstruct_block(
                &relayer.shared().snapshot(),
                &compact,
                transactions,
                &[],
                &[]
            ),
            Err(ReconstructionError::MissingIndexes(missing, vec![])),
        );
    }
}

#[test]
fn test_reconstruct_transactions_and_uncles() {
    let (relayer, always_success_out_point) = build_chain(5);
    let prepare: Vec<TransactionView> = (0..20)
        .map(|i| new_transaction(&relayer, i, &always_success_out_point))
        .collect();

    let (short_transactions, prefilled) = {
        let short_transactions: Vec<TransactionView> = prepare.iter().step_by(2).cloned().collect();
        let prefilled: HashSet<usize> = prepare
            .iter()
            .enumerate()
            .skip(1)
            .step_by(2)
            .map(|(i, _)| i)
            .collect();
        (short_transactions, prefilled)
    };

    let uncle = packed::BlockBuilder::default().build();
    // BLOCK_VALID
    let ext = packed::BlockExtBuilder::default()
        .verified(Some(true).pack())
        .build();

    let block = BlockBuilder::default()
        .transactions(prepare.into_iter().map(|v| v.data()).pack())
        .uncles(vec![uncle.clone().as_uncle()].pack())
        .build();

    let hash = block.calc_header_hash();

    let compact = packed::CompactBlock::build_from_block(&block.into_view(), &prefilled);

    // Should reconstruct block successfully with pool txs
    let (pool_transactions, short_transactions) = short_transactions.split_at(2);
    let short_transactions: Vec<TransactionView> = short_transactions.to_vec();
    let entries = pool_transactions
        .iter()
        .cloned()
        .map(|tx| TxEntry::new(tx.clone(), 0, Capacity::shannons(0), 0, vec![]))
        .collect();
    relayer
        .shared
        .shared()
        .tx_pool_controller()
        .plug_entry(entries, PlugTarget::Pending)
        .unwrap();

    {
        let uncle_view = uncle.into_view();
        let db_txn = relayer.shared().shared().store().begin_transaction();
        db_txn.insert_block(&uncle_view).unwrap();
        db_txn.attach_block(&uncle_view).unwrap();
        db_txn.insert_block_ext(&hash, &ext.unpack()).unwrap();
        db_txn.commit().unwrap();
    }

    assert!(relayer
        .reconstruct_block(
            &relayer.shared().snapshot(),
            &compact,
            short_transactions.clone(),
            &[],
            &[]
        )
        .is_ok());
}

#[test]
fn test_reconstruct_invalid_uncles() {
    let (relayer, _) = build_chain(5);

    let uncle = packed::BlockBuilder::default().build();
    // BLOCK_VALID
    let ext = packed::BlockExtBuilder::default()
        .verified(Some(false).pack())
        .build();

    let block = BlockBuilder::default()
        .uncles(vec![uncle.clone().as_uncle()].pack())
        .build();

    let hash = block.calc_header_hash();

    let compact = packed::CompactBlock::build_from_block(&block.into_view(), &Default::default());

    {
        let uncle_view = uncle.into_view();
        let db_txn = relayer.shared().shared().store().begin_transaction();
        db_txn.insert_block(&uncle_view).unwrap();
        db_txn.attach_block(&uncle_view).unwrap();
        db_txn.insert_block_ext(&hash, &ext.unpack()).unwrap();
        db_txn.commit().unwrap();
    }

    assert_eq!(
        relayer.reconstruct_block(&relayer.shared().snapshot(), &compact, vec![], &[], &[]),
        Err(ReconstructionError::InvalidUncle)
    );
}
