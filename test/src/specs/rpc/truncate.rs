use crate::util::cell::gen_spendable;
use crate::util::transaction::always_success_transactions;
use crate::{Node, Spec};

pub struct RpcTruncate;

impl Spec for RpcTruncate {
    // After truncating, the chain will be rollback to the target block, and tx-pool be cleared.
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];

        let cells = gen_spendable(node, 2);
        let transactions = always_success_transactions(node, &cells);
        let tx1 = &transactions[0];
        let tx2 = &transactions[1];

        let truncate_number = node.get_tip_block_number();

        let to_truncate = node.get_block_by_number(truncate_number).hash();

        node.submit_transaction(tx1);
        node.mine_until_transaction_confirm(&tx1.hash());
        node.submit_transaction(tx2);

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

        node.wait_for_tx_pool();
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
        node.mine(3);
        node.submit_transaction(tx1);
        node.mine_until_transaction_confirm(&tx1.hash());
        let cell1 = node
            .rpc_client()
            .get_live_cell(tx1.inputs().get(0).unwrap().previous_output().into(), false);
        assert_eq!(cell1.status, "unknown", "cell1 was spent within tx1");
    }
}
