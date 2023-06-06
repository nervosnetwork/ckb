use crate::util::check::{
    is_transaction_committed, is_transaction_pending, is_transaction_rejected,
};
use crate::utils::{assert_send_transaction_fail, blank, commit, propose};
use crate::{Node, Spec};
use ckb_types::bytes::Bytes;
use ckb_types::core::{capacity_bytes, Capacity, TransactionView};
use ckb_types::prelude::*;

// Convention:
//   * `tx1` and `tx2` are cousin transactions, with the same transaction content, except the
//   witnesses. Hence `tx1` and `tx2` have the same tx_hash/proposal-id but different witness_hash.

pub struct TransactionHashCollisionDifferentWitnessHashes;

impl Spec for TransactionHashCollisionDifferentWitnessHashes {
    // Case: `tx1` and `tx2` have the same tx_hash, but different witness_hash.
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let window = node.consensus().tx_proposal_window();
        let start_issue = window.farthest() + 2;
        node.mine(start_issue.saturating_sub(node.get_tip_block_number()));

        let (tx1, tx2) = cousin_txs_with_same_hash_different_witness_hash(node);

        // Prepare Phase: Send both `tx1` and `tx2` into pool
        node.submit_transaction(&tx1);
        let result = node.rpc_client().send_transaction_result(tx2.data().into());

        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("PoolRejectedDuplicatedTransaction"));
    }
}

pub struct DuplicatedTransaction;

impl Spec for DuplicatedTransaction {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let window = node.consensus().tx_proposal_window();
        let start_issue = window.farthest() + 2;
        node.mine(start_issue.saturating_sub(node.get_tip_block_number()));

        let tx1 = node.new_transaction_spend_tip_cellbase();

        node.submit_transaction(&tx1);
        let result = node.rpc_client().send_transaction_result(tx1.data().into());

        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("PoolRejectedDuplicatedTransaction"));
    }
}

pub struct ConflictInPending;

impl Spec for ConflictInPending {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let window = node.consensus().tx_proposal_window();
        node.mine(window.farthest() + 2);

        let (txa, txb) = conflict_transactions(node);
        node.submit_transaction(&txa);
        let res = node.submit_transaction_with_result(&txb);
        assert!(res.is_err());

        node.submit_block(&propose(node, &[&txa]));
        (0..window.closest()).for_each(|_| {
            node.submit_block(&blank(node));
        });

        node.submit_block(&commit(node, &[&txa]));
        node.mine(window.farthest());
    }
}

pub struct ConflictInGap;

impl Spec for ConflictInGap {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let window = node.consensus().tx_proposal_window();
        node.mine(window.farthest() + 2);

        let (txa, txb) = conflict_transactions(node);
        node.submit_transaction(&txa);
        let res = node.submit_transaction_with_result(&txb);
        assert!(res.is_err());

        node.submit_block(&propose(node, &[&txa]));
        (0..window.closest() - 1).for_each(|_| {
            node.submit_block(&blank(node));
        });
        node.submit_block(&propose(node, &[&txb]));

        let block = node.new_block(None, None, None);
        assert_eq!(&[txa], &block.transactions()[1..]);

        node.submit_block(&block);
        node.mine(window.farthest());
    }
}

pub struct ConflictInProposed;

impl Spec for ConflictInProposed {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let window = node.consensus().tx_proposal_window();
        node.mine(window.farthest() + 2);

        let (txa, txb) = conflict_transactions(node);
        node.submit_transaction(&txa);
        let res = node.submit_transaction_with_result(&txb);
        assert!(res.is_err());

        node.submit_block(&propose(node, &[&txa, &txb]));
        node.mine(window.farthest());
    }
}

pub struct SubmitConflict;

impl Spec for SubmitConflict {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let window = node.consensus().tx_proposal_window();
        node.mine(window.farthest() + 2);

        let (txa, txb) = conflict_transactions(node);
        node.submit_transaction(&txa);
        node.mine_until_transaction_confirm(&txa.hash());
        assert!(is_transaction_committed(node, &txa));
        assert_send_transaction_fail(
            node,
            &txb,
            "TransactionFailedToResolve: Resolve failed Unknown",
        );
    }
}

pub struct RemoveConflictFromPending;

impl Spec for RemoveConflictFromPending {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let window = node.consensus().tx_proposal_window();
        node.mine(window.farthest() + 2);

        let (txa, txb) =
            conflict_transactions_with_capacity(node, Bytes::new(), capacity_bytes!(1000));
        let txc = node.new_transaction_with_since_capacity(txb.hash(), 0, capacity_bytes!(100));
        node.submit_transaction(&txa);
        let res = node.submit_transaction_with_result(&txb);
        assert!(res.is_err());

        let res = node.submit_transaction_with_result(&txc);
        assert!(res.is_err());

        assert!(is_transaction_pending(node, &txa));
        assert!(is_transaction_rejected(node, &txb));
        assert!(is_transaction_rejected(node, &txc));

        node.submit_block(&propose(node, &[&txa]));
        (0..window.closest()).for_each(|_| {
            node.submit_block(&blank(node));
        });
        node.submit_block(&commit(node, &[&txa]));
        node.wait_for_tx_pool();

        assert!(is_transaction_committed(node, &txa));
        assert!(is_transaction_rejected(node, &txb));
        assert!(is_transaction_rejected(node, &txc));
    }
}

fn conflict_transactions_with_capacity(
    node: &Node,
    output_data: Bytes,
    cap: Capacity,
) -> (TransactionView, TransactionView) {
    let txa = node.new_transaction_spend_tip_cellbase();
    let output = txa
        .output(0)
        .unwrap()
        .as_builder()
        .build_exact_capacity(cap)
        .unwrap();
    let txb = txa
        .as_advanced_builder()
        .set_outputs_data(vec![output_data.pack()])
        .set_outputs(vec![output])
        .build();
    assert_ne!(txa.hash(), txb.hash());
    (txa, txb)
}

fn conflict_transactions(node: &Node) -> (TransactionView, TransactionView) {
    let output_data = Bytes::from(b"b0b".to_vec());
    let cap = Capacity::bytes(output_data.len()).unwrap();
    conflict_transactions_with_capacity(node, output_data, cap)
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
