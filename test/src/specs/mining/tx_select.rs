use crate::generic::GetCommitTxIds;
use crate::util::cell::gen_spendable;
use crate::util::mining::mine;
use crate::util::transaction::always_success_transaction;
use crate::{Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_types::bytes::Bytes;
use ckb_types::prelude::*;

pub struct TemplateTxSelect;

impl Spec for TemplateTxSelect {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let cells = gen_spendable(node, 5);
        let txs: Vec<_> = cells
            .iter()
            .zip(vec![501, 501, 501, 501, 300].into_iter())
            .map(|(cell, n)| {
                let tx = always_success_transaction(node, cell);
                let original_tx_size = tx.data().serialized_size_in_block();
                let expect_tx_size = n;
                let data_size = expect_tx_size - original_tx_size;
                let output_data = Bytes::from(vec![0u8; data_size]).pack();
                tx.as_advanced_builder()
                    .set_outputs_data(vec![output_data])
                    .build()
            })
            .collect();

        let blank_block_size = node
            .get_tip_block()
            .data()
            .serialized_size_without_uncle_proposals();

        // send transactions and skip proposal window
        txs.iter().for_each(|tx| {
            node.submit_transaction(tx);
        });
        mine(node, DEFAULT_TX_PROPOSAL_WINDOW.0);

        let new_block = node.new_block(Some(blank_block_size as u64 + 900), None, None);
        assert_eq!(
            new_block.get_commit_tx_ids().len(),
            2,
            "New block should contain txs: 501, 300"
        );
    }
}
