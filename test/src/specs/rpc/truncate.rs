use crate::{Node, Spec};

pub struct RpcTruncate;

impl Spec for RpcTruncate {
    // After truncating, the chain will be rollback to the target block, and tx-pool be cleared.
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        node.generate_blocks(12);
        let to_truncate = node.get_block_by_number(node.get_tip_block_number()).hash();
        let tx1 = {
            let tx1 = node.new_transaction_spend_tip_cellbase();
            node.submit_transaction(&tx1);
            tx1
        };
        node.generate_blocks(3);
        let _tx2 = {
            let tx2 = node.new_transaction_spend_tip_cellbase();
            node.submit_transaction(&tx2);
            tx2
        };

        // tx1 is already committed on chain, tx2 is still in tx-pool.

        let cell1 = node
            .rpc_client()
            .get_live_cell(tx1.inputs().get(0).unwrap().previous_output().into(), false);
        assert_eq!(cell1.status, "unknown", "cell1 was spent within tx1");

        let tx_pool_info = node.get_tip_tx_pool_info();
        assert!(tx_pool_info.total_tx_size.value() > 0, "tx-pool holds tx2");

        // Truncate from `to_truncate`

        let old_tip_block = node.get_tip_block();
        node.rpc_client().truncate(to_truncate.clone());

        // After truncating, tx1 has been rollback, so cell1 become alive.

        assert_eq!(node.get_tip_block().hash(), to_truncate);
        assert!(
            node.rpc_client().get_header(old_tip_block.hash()).is_none(),
            "old_tip_block should be truncated",
        );
        assert!(
            node.rpc_client()
                .get_block_by_number(old_tip_block.number())
                .is_none(),
            "old_tip_block should be truncated",
        );

        let cell1 = node
            .rpc_client()
            .get_live_cell(tx1.inputs().get(0).unwrap().previous_output().into(), false);
        assert_eq!(cell1.status, "live", "cell1 is alive after roll-backing");

        let tx_pool_info = node.get_tip_tx_pool_info();
        assert_eq!(tx_pool_info.orphan.value(), 0, "tx-pool was cleared");
        assert_eq!(tx_pool_info.pending.value(), 0, "tx-pool was cleared");
        assert_eq!(tx_pool_info.proposed.value(), 0, "tx-pool was cleared");
        assert_eq!(
            tx_pool_info.total_tx_cycles.value(),
            0,
            "tx-pool was cleared"
        );
        assert_eq!(tx_pool_info.total_tx_size.value(), 0, "tx-pool was cleared");

        // The chain can generate new blocks
        node.generate_blocks(3);
        node.submit_transaction(&tx1);
        node.generate_blocks(3);
        let cell1 = node
            .rpc_client()
            .get_live_cell(tx1.inputs().get(0).unwrap().previous_output().into(), false);
        assert_eq!(cell1.status, "unknown", "cell1 was spent within tx1");
    }
}
