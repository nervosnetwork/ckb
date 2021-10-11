use crate::util::{mining::mine_until_out_bootstrap_period, transaction::relay_tx};
use crate::utils::wait_until;
use crate::{Net, Node, Spec};
use ckb_network::SupportProtocols;

const ALWAYS_SUCCESS_SCRIPT_CYCLE: u64 = 537;

pub struct DeclaredWrongCycles;

impl Spec for DeclaredWrongCycles {
    crate::setup!(num_nodes: 1);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &mut nodes[0];
        mine_until_out_bootstrap_period(node0);

        let mut net = Net::new(
            self.name(),
            node0.consensus(),
            vec![SupportProtocols::Relay],
        );
        net.connect(node0);

        let tx = node0.new_transaction_spend_tip_cellbase();

        relay_tx(&net, &node0, tx, ALWAYS_SUCCESS_SCRIPT_CYCLE + 1);

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
        mine_until_out_bootstrap_period(node0);

        let mut net = Net::new(
            self.name(),
            node0.consensus(),
            vec![SupportProtocols::Relay],
        );
        net.connect(node0);

        let tx = node0.new_transaction_spend_tip_cellbase();

        relay_tx(&net, &node0, tx, ALWAYS_SUCCESS_SCRIPT_CYCLE + 1);

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
