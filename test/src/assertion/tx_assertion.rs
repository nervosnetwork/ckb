use crate::Node;
use ckb_types::core::{BlockView, TransactionView};
use ckb_types::prelude::*;
use std::collections::HashSet;

pub fn assert_proposed_txs(block: &BlockView, expected: &[TransactionView]) {
    let mut actual_proposals: Vec<_> = block.union_proposal_ids_iter().collect();
    let mut expected_proposals: Vec<_> = expected.iter().map(|tx| tx.proposal_short_id()).collect();
    actual_proposals.sort_by(|a, b| a.as_bytes().cmp(&b.as_bytes()));
    expected_proposals.sort_by(|a, b| a.as_bytes().cmp(&b.as_bytes()));
    assert_eq!(
        expected_proposals, actual_proposals,
        "assert_proposed_txs failed, expected: {:?}, actual: {:?}",
        expected_proposals, actual_proposals
    );
}

pub fn assert_committed_txs(block: &BlockView, expected: &[TransactionView]) {
    let actual_committed_hashes: Vec<_> = block
        .transactions()
        .iter()
        .skip(1)
        .map(|tx| tx.hash())
        .collect();
    let expected_committed_hashes: Vec<_> = expected.iter().map(|tx| tx.hash()).collect();
    assert_eq!(
        &expected_committed_hashes, &actual_committed_hashes,
        "assert_committed_txs failed, expected: {:?}, actual: {:?}",
        expected_committed_hashes, actual_committed_hashes
    );
}

// Check the given transactions were committed
pub fn assert_transactions_committed(node: &Node, transactions: &[TransactionView]) {
    let tip_number = node.get_tip_block_number();
    let mut hashes: HashSet<_> = transactions.iter().map(|tx| tx.hash()).collect();
    (1..tip_number).for_each(|number| {
        let block = node.get_block_by_number(number);
        block.transactions().iter().skip(1).for_each(|tx| {
            hashes.remove(&tx.hash());
        });
    });
    assert!(hashes.is_empty());
}

// Check the given transaction aren't committed
pub fn assert_transaction_not_committed(node: &Node, transaction: &TransactionView) {
    let tip_number = node.get_tip_block_number();
    let tx_hash = transaction.hash();
    (1..tip_number).for_each(|number| {
        let block = node.get_block_by_number(number);
        assert!(!block
            .transactions()
            .iter()
            .any(|tx| { tx.hash() == tx_hash }));
    });
}
