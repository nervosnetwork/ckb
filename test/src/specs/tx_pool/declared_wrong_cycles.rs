use crate::util::{mining::out_ibd_mode, transaction::relay_tx};
use crate::utils::{sleep, wait_until};
use crate::{Net, Node, Spec};
use ckb_network::SupportProtocols;

const ALWAYS_SUCCESS_SCRIPT_CYCLE: u64 = 537;

pub struct DeclaredWrongCycles;

impl Spec for DeclaredWrongCycles {
    crate::setup!(num_nodes: 1);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &mut nodes[0];
        node0.mine_until_out_bootstrap_period();

        let mut net = Net::new(
            self.name(),
            node0.consensus(),
            vec![SupportProtocols::RelayV3],
        );
        net.connect(node0);

        let tx = node0.new_transaction_spend_tip_cellbase();

        relay_tx(&net, node0, tx, ALWAYS_SUCCESS_SCRIPT_CYCLE + 1);

        let result = wait_until(5, || {
            let tx_pool_info = node0.get_tip_tx_pool_info();
            tx_pool_info.orphan.value() == 0 && tx_pool_info.pending.value() == 0
        });
        assert!(result, "Declared wrong cycles should be rejected");
    }
}

pub struct DeclaredWrongCyclesChunk;

impl Spec for DeclaredWrongCyclesChunk {
    crate::setup!(num_nodes: 1);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &mut nodes[0];
        node0.mine_until_out_bootstrap_period();

        let mut net = Net::new(
            self.name(),
            node0.consensus(),
            vec![SupportProtocols::RelayV3],
        );
        net.connect(node0);

        let tx = node0.new_transaction_spend_tip_cellbase();

        relay_tx(&net, node0, tx, ALWAYS_SUCCESS_SCRIPT_CYCLE + 1);

        let result = wait_until(5, || {
            let tx_pool_info = node0.get_tip_tx_pool_info();
            tx_pool_info.orphan.value() == 0 && tx_pool_info.pending.value() == 0
        });
        assert!(result, "Declared wrong cycles should be rejected");
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.network.connect_outbound_interval_secs = 0;
        config.tx_pool.max_tx_verify_cycles = 500; // ALWAYS_SUCCESS_SCRIPT_CYCLE: u64 = 537
    }
}

pub struct DeclaredWrongCyclesAndRelayAgain;

impl Spec for DeclaredWrongCyclesAndRelayAgain {
    crate::setup!(num_nodes: 3);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];
        node0.mine_until_out_bootstrap_period();
        out_ibd_mode(nodes);

        let mut net = Net::new(
            self.name(),
            node0.consensus(),
            vec![SupportProtocols::RelayV3],
        );

        let tx = node0.new_transaction_spend_tip_cellbase();
        // relay tx to node0 with wrong cycles
        net.connect(node0);
        relay_tx(&net, node0, tx.clone(), ALWAYS_SUCCESS_SCRIPT_CYCLE + 1);
        let ret = wait_until(10, || node0.rpc_client().get_peers().is_empty());
        assert!(
            ret,
            "The address of net should be removed from node0's peers",
        );
        // connect node0 and node2, make sure node0's relay tx hash processing is working
        node0.rpc_client().clear_banned_addresses();
        node0.connect(node2);
        // removing invalid tx hash from node0's known tx filer is async, wait 5 seconds to make sure it's removed
        sleep(5);

        // connect node0 with node1, tx will be relayed from node1 to node0
        node0.connect(node1);

        // relay tx to node1 with correct cycles
        net.connect(node1);
        relay_tx(&net, node1, tx, ALWAYS_SUCCESS_SCRIPT_CYCLE);

        let result = wait_until(5, || {
            let tx_pool_info = node0.get_tip_tx_pool_info();
            tx_pool_info.orphan.value() == 0 && tx_pool_info.pending.value() == 1
        });
        assert!(
            result,
            "Tx with wrong cycles should be relayed again with correct cycle"
        );
    }
}
