use crate::{Node, Spec};

pub struct TxPoolEntryStatus;

impl Spec for TxPoolEntryStatus {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();
        node0.new_block_with_blocking(|template| template.number.value() != 13);
        let tx_hash_0 = node0.generate_transaction();
        let tx = node0.new_transaction(tx_hash_0.clone());
        node0.rpc_client().send_transaction(tx.data().into());
        node0.assert_pool_entry_status(tx_hash_0.clone(), "Pending");
        node0.mine(1);
        node0.assert_pool_entry_status(tx_hash_0.clone(), "Gap");
        node0.mine(1);
        node0.assert_pool_entry_status(tx_hash_0, "Proposed");
    }
}
