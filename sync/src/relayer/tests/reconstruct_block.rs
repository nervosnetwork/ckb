use super::helper::{build_chain, new_transaction};
use crate::StatusCode;
use crate::relayer::ReconstructionResult;
use crate::relayer::packed::{CellInput, OutPoint};
use ckb_tx_pool::{PlugTarget, TxEntry};
use ckb_types::prelude::*;
use ckb_types::{
    core::{BlockBuilder, Capacity, TransactionView},
    packed::{self, CompactBlockBuilder},
};
use std::collections::HashSet;
// There are more test cases in block_transactions_process and compact_block_process.rs
#[test]
fn test_missing_txs() {
    let (_chain, relayer, always_success_out_point) = build_chain(5);
    let prepare: Vec<TransactionView> = (0..20)
        .map(|i| new_transaction(&relayer, i, &always_success_out_point))
        .collect();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // Case: miss tx.0
    {
        let compact_block_builder = CompactBlockBuilder::default();
        let short_ids = prepare
            .iter()
            .map(|tx| tx.proposal_short_id())
            .collect::<Vec<_>>();
        let transactions: Vec<TransactionView> = prepare.iter().skip(1).cloned().collect();
        let compact = compact_block_builder.short_ids(short_ids).build();
        assert_eq!(
            rt.block_on(relayer.reconstruct_block(
                &relayer.shared().active_chain(),
                &compact,
                transactions,
                &[],
                &[]
            )),
            ReconstructionResult::Missing(vec![0], vec![]),
        );
    }

    // Case: miss multiple txs
    {
        let compact_block_builder = CompactBlockBuilder::default();
        let short_ids = prepare
            .iter()
            .map(|tx| tx.proposal_short_id())
            .collect::<Vec<_>>();
        let transactions: Vec<TransactionView> =
            prepare.iter().skip(1).step_by(2).cloned().collect();
        let missing = prepare
            .iter()
            .enumerate()
            .step_by(2)
            .map(|(i, _)| i)
            .collect();
        let compact = compact_block_builder.short_ids(short_ids).build();
        assert_eq!(
            rt.block_on(relayer.reconstruct_block(
                &relayer.shared().active_chain(),
                &compact,
                transactions,
                &[],
                &[]
            )),
            ReconstructionResult::Missing(missing, vec![]),
        );
    }
}

#[test]
fn test_reconstruct_transactions_and_uncles() {
    let (_chain, relayer, always_success_out_point) = build_chain(5);
    let parent = new_transaction(&relayer, 0, &always_success_out_point);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // create a chain of transactions as prepare
    let mut prepare = vec![parent];
    while prepare.len() <= 20 {
        let parent = prepare.last().unwrap();
        let child = parent
            .as_advanced_builder()
            .set_inputs(vec![{
                CellInput::new_builder()
                    .previous_output(OutPoint::new(parent.hash(), 0))
                    .build()
            }])
            .set_outputs(vec![parent.output(0).unwrap()])
            .build();
        prepare.push(child);
    }
    let uncle = BlockBuilder::default().build();

    let block = BlockBuilder::default()
        .transactions(prepare.clone())
        .uncles(vec![uncle.as_uncle()])
        .build();

    let uncle_hash = uncle.hash();

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

    // BLOCK_VALID
    let ext = packed::BlockExtBuilder::default()
        .verified(Some(true))
        .build();

    let compact = packed::CompactBlock::build_from_block(&block, &prefilled);

    // Should reconstruct block successfully with pool txs
    let (pool_transactions, short_transactions) = short_transactions.split_at(2);
    let short_transactions: Vec<TransactionView> = short_transactions.to_vec();
    let entries = pool_transactions
        .iter()
        .cloned()
        .map(|tx| TxEntry::dummy_resolve(tx, 0, Capacity::shannons(0), 0))
        .collect();
    relayer
        .shared
        .shared()
        .tx_pool_controller()
        .plug_entry(entries, PlugTarget::Pending)
        .unwrap();

    {
        let db_txn = relayer.shared().shared().store().begin_transaction();
        db_txn.insert_block(&uncle).unwrap();
        db_txn.insert_block_ext(&uncle_hash, &ext.into()).unwrap();
        db_txn.commit().unwrap();
    }
    relayer.shared().shared().refresh_snapshot();

    let ret = rt.block_on(relayer.reconstruct_block(
        &relayer.shared().active_chain(),
        &compact,
        short_transactions,
        &[],
        &[],
    ));
    assert_eq!(ret, ReconstructionResult::Block(block), "{ret:?}");
}

#[test]
fn test_reconstruct_invalid_uncles() {
    let (_chain, relayer, _) = build_chain(5);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let uncle = BlockBuilder::default().build();
    // BLOCK_VALID
    let ext = packed::BlockExtBuilder::default()
        .verified(Some(false))
        .build();

    let block = BlockBuilder::default()
        .uncles(vec![uncle.as_uncle()])
        .build();

    let uncle_hash = uncle.hash();
    let compact = packed::CompactBlock::build_from_block(&block, &Default::default());

    {
        let db_txn = relayer.shared().shared().store().begin_transaction();
        db_txn.insert_block(&uncle).unwrap();
        db_txn.attach_block(&uncle).unwrap();
        db_txn.insert_block_ext(&uncle_hash, &ext.into()).unwrap();
        db_txn.commit().unwrap();
    }
    relayer.shared().shared().refresh_snapshot();

    assert_eq!(
        rt.block_on(relayer.reconstruct_block(
            &relayer.shared().active_chain(),
            &compact,
            vec![],
            &[],
            &[]
        )),
        ReconstructionResult::Error(StatusCode::CompactBlockHasInvalidUncle.into()),
    );
}
