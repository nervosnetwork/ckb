use crate::{Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_types::{
    core::{capacity_bytes, Capacity, TransactionView},
    packed::CellOutputBuilder,
    prelude::*,
};
use log::info;

pub struct DifferentTxsWithSameInput;

impl Spec for DifferentTxsWithSameInput {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);
        let tx_hash_0 = node0.generate_transaction();
        info!("Generate 2 txs with same input");
        let tx1 = node0.new_transaction(tx_hash_0.clone());
        let tx2_temp = node0.new_transaction(tx_hash_0);
        // Set tx2 fee to a higher value, tx1 capacity is 100, set tx2 capacity to 80 for +20 fee.
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(80).pack())
            .build();

        let tx2 = tx2_temp
            .as_advanced_builder()
            .set_outputs(vec![output])
            .build();
        node0.rpc_client().send_transaction(tx1.data().into());
        node0.rpc_client().send_transaction(tx2.data().into());

        node0.generate_block();
        node0.generate_block();

        info!("RBF (Replace-By-Fees) is not implemented, but transaction fee sorting is ready");
        info!("tx2 should be included in the next + 2 block, and tx1 should be ignored");
        node0.generate_block();
        let tip_block = node0.get_tip_block();
        let commit_txs_hash: Vec<_> = tip_block
            .transactions()
            .iter()
            .map(TransactionView::hash)
            .collect();

        assert!(commit_txs_hash.contains(&tx2.hash()));
        assert!(!commit_txs_hash.contains(&tx1.hash()));
    }
}
