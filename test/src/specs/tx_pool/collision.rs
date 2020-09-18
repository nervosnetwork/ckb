use crate::utils::{blank, commit, propose};
use crate::{Node, Spec};
use ckb_types::bytes::Bytes;
use ckb_types::core::{Capacity, TransactionView};
use ckb_types::prelude::*;

// Convention:
//   * `tx1` and `tx2` are cousin transactions, with the same transaction content, expect the
//   witnesses. Hence `tx1` and `tx2` have the same tx_hash/proposal-id but different witness_hash.

pub struct TransactionHashCollisionDifferentWitnessHashes;

impl Spec for TransactionHashCollisionDifferentWitnessHashes {
    // Case: `tx1` and `tx2` have the same tx_hash, but different witness_hash.
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
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
            .contains("PoolRejectedDuplicatedTransaction"));
    }
}

pub struct DuplicatedTransaction;

impl Spec for DuplicatedTransaction {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
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
            .contains("PoolRejectedDuplicatedTransaction"));
    }
}

pub struct ConflictInPending;

impl Spec for ConflictInPending {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let window = node.consensus().tx_proposal_window();
        node.generate_blocks(window.farthest() as usize + 2);

        let (txa, txb) = conflict_transactions(node);
        node.submit_transaction(&txa);
        node.submit_transaction(&txb);

        node.submit_block(&propose(node, &[&txa]));
        (0..window.closest()).for_each(|_| {
            node.submit_block(&blank(node));
        });

        node.submit_block(&commit(node, &[&txa]));
        node.generate_blocks(window.farthest() as usize);
    }
}

pub struct ConflictInGap;

impl Spec for ConflictInGap {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let window = node.consensus().tx_proposal_window();
        node.generate_blocks(window.farthest() as usize + 2);

        let (txa, txb) = conflict_transactions(node);
        node.submit_transaction(&txa);
        node.submit_transaction(&txb);

        node.submit_block(&propose(node, &[&txa]));
        (0..window.closest() - 1).for_each(|_| {
            node.submit_block(&blank(node));
        });
        node.submit_block(&propose(node, &[&txb]));
        let block = node.new_block(None, None, None);
        assert_eq!(&[txa], &block.transactions()[1..]);

        node.submit_block(&block);
        node.generate_blocks(window.farthest() as usize);
    }
}

pub struct ConflictInProposed;

impl Spec for ConflictInProposed {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let window = node.consensus().tx_proposal_window();
        node.generate_blocks(window.farthest() as usize + 2);

        let (txa, txb) = conflict_transactions(node);
        node.submit_transaction(&txa);
        node.submit_transaction(&txb);

        node.submit_block(&propose(node, &[&txa, &txb]));
        node.generate_blocks(window.farthest() as usize);
    }
}

fn conflict_transactions(node: &Node) -> (TransactionView, TransactionView) {
    let txa = node.new_transaction_spend_tip_cellbase();
    let output_data = Bytes::from(b"b0b".to_vec());
    let output = txa
        .output(0)
        .unwrap()
        .as_builder()
        .build_exact_capacity(Capacity::bytes(output_data.len()).unwrap())
        .unwrap();
    let txb = txa
        .as_advanced_builder()
        .set_outputs_data(vec![output_data.pack()])
        .set_outputs(vec![output])
        .build();
    assert_ne!(txa.hash(), txb.hash());
    (txa, txb)
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
