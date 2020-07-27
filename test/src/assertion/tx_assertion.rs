use crate::Node;
use ckb_types::core::TransactionView;
use std::collections::HashSet;

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
