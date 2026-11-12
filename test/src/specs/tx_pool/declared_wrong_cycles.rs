use crate::util::{mining::out_ibd_mode, transaction::relay_tx};
use crate::utils::{sleep, wait_until};
use crate::{Net, Node, Spec};
use ckb_network::SupportProtocols;

const ALWAYS_SUCCESS_SCRIPT_CYCLE: u64 = 537;

fn wait_for_wrong_cycles_rejection(node: &Node, hash: ckb_types::packed::Byte32) {
    // An initially empty pool is not evidence that the submitted body was
    // verified. Wait for its terminal rejection before checking cleanup.
    assert!(
        wait_until(10, || {
            let status = node.rpc_client().get_transaction(hash.clone()).tx_status;
            status.status == ckb_jsonrpc_types::Status::Rejected
                && status
                    .reason
                    .is_some_and(|reason| reason.contains("DeclaredWrongCycles"))
        }),
        "wrong declared cycles did not produce a terminal rejection"
    );
    assert!(
        wait_until(10, || !node.rpc_client().get_banned_addresses().is_empty()),
        "the malformed cycle declaration must ban its source"
    );
    let info = node.get_tip_tx_pool_info();
    assert_eq!(info.pending.value(), 0);
    assert_eq!(info.orphan.value(), 0);
    assert_eq!(info.verify_queue_size.value(), 0);
}

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
        let hash = tx.hash();
        relay_tx(&net, node0, tx, ALWAYS_SUCCESS_SCRIPT_CYCLE + 1);
        wait_for_wrong_cycles_rejection(node0, hash);
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
        let hash = tx.hash();
        relay_tx(&net, node0, tx, ALWAYS_SUCCESS_SCRIPT_CYCLE + 1);
        wait_for_wrong_cycles_rejection(node0, hash);
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
        wait_for_wrong_cycles_rejection(node0, tx.hash());
        let ret = wait_until(10, || node0.rpc_client().get_peers().is_empty());
        assert!(
            ret,
            "The address of net should be removed from node0's peers",
        );
        // connect node0 and node2, make sure node0's relay tx hash processing is working
        node0.rpc_client().clear_banned_addresses();
        node0.connect(node2);
        // The relayer consumes the filter reset asynchronously. A visible ban
        // does not acknowledge that consumer; retain its existing settling time.
        sleep(5);
        // connect node0 with node1, tx will be relayed from node1 to node0
        node0.connect(node1);

        // relay tx to node1 with correct cycles
        net.connect(node1);
        let hash = tx.hash();
        relay_tx(&net, node1, tx, ALWAYS_SUCCESS_SCRIPT_CYCLE);

        let result = wait_until(30, || {
            let tx_pool_info = node0.get_tip_tx_pool_info();
            tx_pool_info.orphan.value() == 0
                && tx_pool_info.pending.value() == 1
                && node0.get_transaction(hash.clone()) == ckb_jsonrpc_types::TxStatus::pending()
        });
        assert!(
            result,
            "Tx with wrong cycles should be relayed again with correct cycle"
        );
    }
}
