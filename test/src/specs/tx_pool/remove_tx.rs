use ckb_network::SupportProtocols;

use crate::{
    util::{check, transaction::relay_tx},
    utils::wait_until,
    Net, Node, Spec,
};

const ALWAYS_SUCCESS_SCRIPT_CYCLE: u64 = 537;

pub struct RemoveTx;

impl Spec for RemoveTx {
    crate::setup!(num_nodes: 1);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &mut nodes[0];
        node0.mine_until_out_bootstrap_period();

        let mut net = Net::new(
            self.name(),
            node0.consensus(),
            vec![SupportProtocols::Relay],
        );
        net.connect(node0);

        {
            // Remove a tx from orphan pool
            let parent_tx = node0.new_transaction_spend_tip_cellbase();
            let child_tx = node0.new_transaction(parent_tx.hash());

            node0.assert_tx_pool_statics(0, 0);

            relay_tx(&net, node0, child_tx.clone(), ALWAYS_SUCCESS_SCRIPT_CYCLE);
            let result = wait_until(5, || {
                let tx_pool_info = node0.get_tip_tx_pool_info();
                tx_pool_info.orphan.value() == 1 && tx_pool_info.pending.value() == 0
            });
            assert!(
                result,
                "Send child tx first, it will be added to orphan tx pool"
            );
            node0.assert_tx_pool_statics(0, 0);

            node0.remove_transaction(child_tx.hash());
            let result = wait_until(5, || {
                let tx_pool_info = node0.get_tip_tx_pool_info();
                tx_pool_info.orphan.value() == 0 && tx_pool_info.pending.value() == 0
            });
            assert!(result, "remove a tx from orphan tx pool");
            node0.assert_tx_pool_statics(0, 0);
        }

        let tx = node0.new_transaction_spend_tip_cellbase();

        let (tx_size, tx_cycles) = {
            // Remove a tx from pending pool
            node0.assert_tx_pool_statics(0, 0);

            node0.submit_transaction(&tx);
            let result = wait_until(5, || {
                let tx_pool_info = node0.get_tip_tx_pool_info();
                tx_pool_info.pending.value() == 1 && tx_pool_info.proposed.value() == 0
            });
            assert!(result, "add a tx into pending tx pool");

            let tx_pool_info = node0.get_tip_tx_pool_info();
            let tx_size = tx_pool_info.total_tx_size.value();
            let tx_cycles = tx_pool_info.total_tx_cycles.value();
            assert!(tx_size > 0 && tx_cycles > 0);

            assert!(check::is_transaction_pending(node0, &tx));

            node0.remove_transaction(tx.hash());
            let result = wait_until(5, || {
                let tx_pool_info = node0.get_tip_tx_pool_info();
                tx_pool_info.pending.value() == 0 && tx_pool_info.proposed.value() == 0
            });
            assert!(result, "remove a tx from pending tx pool");
            node0.assert_tx_pool_statics(0, 0);

            (tx_size, tx_cycles)
        };

        {
            // Remove a tx from proposed pool
            node0.assert_tx_pool_statics(0, 0);

            node0.submit_transaction(&tx);
            let result = wait_until(5, || {
                let tx_pool_info = node0.get_tip_tx_pool_info();
                tx_pool_info.pending.value() == 1 && tx_pool_info.proposed.value() == 0
            });
            assert!(result, "add a tx into pending tx pool");
            assert!(check::is_transaction_pending(node0, &tx));
            node0.assert_tx_pool_statics(tx_size, tx_cycles);

            let proposed = node0.mine_with_blocking(|template| template.proposals.len() != 1);
            node0.mine_with_blocking(|template| template.number.value() != (proposed + 1));
            let result = wait_until(5, || {
                let tx_pool_info = node0.get_tip_tx_pool_info();
                tx_pool_info.pending.value() == 0 && tx_pool_info.proposed.value() == 1
            });
            assert!(result, "add a tx into proposed tx pool");
            assert!(check::is_transaction_proposed(node0, &tx));
            node0.assert_tx_pool_statics(tx_size, tx_cycles);

            node0.remove_transaction(tx.hash());
            let result = wait_until(5, || {
                let tx_pool_info = node0.get_tip_tx_pool_info();
                tx_pool_info.pending.value() == 0 && tx_pool_info.proposed.value() == 0
            });
            assert!(result, "remove a tx from proposed tx pool");
            node0.assert_tx_pool_statics(0, 0);
        }
    }
}
