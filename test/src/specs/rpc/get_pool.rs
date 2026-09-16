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
        node0.assert_pool_entry_status(tx_hash_0.clone(), "pending");
        let proposal: ckb_jsonrpc_types::ProposalShortId =
            ckb_types::packed::ProposalShortId::from_tx_hash(&tx_hash_0).into();
        // Template publication and chain reconciliation are asynchronous.
        // Establish that this block really proposes the transaction before
        // testing the resulting pool projection.
        let block =
            node0.new_block_with_blocking(|template| !template.proposals.contains(&proposal));
        node0.submit_block(&block);
        node0.get_tip_tx_pool_info();
        node0.assert_pool_entry_status(tx_hash_0.clone(), "gap");
        node0.mine(1);
        node0.get_tip_tx_pool_info();
        node0.assert_pool_entry_status(tx_hash_0, "proposed");
    }
}
