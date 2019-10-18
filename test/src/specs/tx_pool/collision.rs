use crate::{Net, Node, Spec};
use ckb_types::core::TransactionView;

// Convention:
//   * `tx1` and `tx2` are cousin transactions, with the same transaction content, expect the
//   witnesses. Hence `tx1` and `tx2` have the same tx_hash/proposal-id but different witness_hash.

pub struct TransactionHashCollisionDifferentWitnessHashes;

impl Spec for TransactionHashCollisionDifferentWitnessHashes {
    crate::name!("transaction_hash_collision_different_witness_hashes_1");

    // Case: `tx1` and `tx2` have the same tx_hash, but different witness_hash.
    fn run(&self, net: &mut Net) {
        let node = &net.nodes[0];
        let window = node.consensus().tx_proposal_window();
        let start_issue = window.farthest() + 2;
        node.generate_blocks((start_issue.saturating_sub(node.get_tip_block_number())) as usize);

        let (tx1, tx2) = cousin_txs_with_same_hash_different_witness_hash(node);

        // Prepare Phase: Send both `tx1` and `tx2` into pool
        node.submit_transaction(&tx1);
        let result = node.rpc_client().send_transaction_result(tx2.data().into());

        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("PoolTransactionDuplicated"));
    }
}

pub struct DuplicatedTransaction;

impl Spec for DuplicatedTransaction {
    crate::name!("duplicated_transaction");

    fn run(&self, net: &mut Net) {
        let node = &net.nodes[0];
        let window = node.consensus().tx_proposal_window();
        let start_issue = window.farthest() + 2;
        node.generate_blocks((start_issue.saturating_sub(node.get_tip_block_number())) as usize);

        let tx1 = node.new_transaction_spend_tip_cellbase();

        node.submit_transaction(&tx1);
        let result = node.rpc_client().send_transaction_result(tx1.data().into());

        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("PoolTransactionDuplicated"));
    }
}

fn cousin_txs_with_same_hash_different_witness_hash(
    node: &Node,
) -> (TransactionView, TransactionView) {
    let tx1 = node.new_transaction_spend_tip_cellbase();
    let tx2 = tx1
        .as_advanced_builder()
        .witness(Default::default())
        .build();
    assert_eq!(tx1.hash(), tx2.hash());
    assert_eq!(tx1.proposal_short_id(), tx2.proposal_short_id());
    assert_ne!(tx1.witness_hash(), tx2.witness_hash());

    (tx1, tx2)
}
