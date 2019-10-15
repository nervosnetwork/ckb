use crate::utils::{blank, commit, propose};
use crate::{Net, Node, Spec};
use ckb_types::bytes::Bytes;
use ckb_types::core::{Capacity, TransactionView};
use ckb_types::prelude::*;

pub struct ConflictInPending;

impl Spec for ConflictInPending {
    crate::name!("conflict_in_pending");

    fn run(&self, net: &mut Net) {
        let node = &net.nodes[0];
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
    crate::name!("conflict_in_gap");

    fn run(&self, net: &mut Net) {
        let node = &net.nodes[0];
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
    crate::name!("conflict_in_proposed");

    fn run(&self, net: &mut Net) {
        let node = &net.nodes[0];
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
