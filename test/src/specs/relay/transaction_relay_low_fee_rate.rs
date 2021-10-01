use crate::node::connect_all;
use crate::node::waiting_for_sync;
use crate::util::cell::{as_input, as_output, gen_spendable};
use crate::util::log_monitor::monitor_log_until_expected_show;
use crate::util::mining::out_ibd_mode;
use crate::{Node, Spec};
use ckb_logger::debug;
use ckb_types::core::{FeeRate, TransactionBuilder};

pub struct TransactionRelayLowFeeRate;

impl Spec for TransactionRelayLowFeeRate {
    fn run(&self, nodes: &mut Vec<Node>) {
        out_ibd_mode(nodes);
        connect_all(nodes);

        let node0 = &nodes[0];
        let node1 = &nodes[1];

        let cells = gen_spendable(node0, 1);
        // As for `low_fee`, which is `inputs.total_capacity == outputs.total_capacity`,
        // so it is a low-fee-rate transaction in this case;
        let low_fee = TransactionBuilder::default()
            .input(as_input(&cells[0]))
            .output(as_output(&cells[0]))
            .output_data(Default::default())
            .cell_dep(node0.always_success_cell_dep())
            .build();

        debug!("make sure node1 has the cell");
        waiting_for_sync(nodes);

        node0.rpc_client().send_transaction(low_fee.data().into());

        assert!(monitor_log_until_expected_show(
            node1,
            0,
            10,
            "reject tx The min fee rate is 1000 shannons/KB, so the transaction fee should be 242 shannons at least, but only got 0"
        )
        .is_some());
    }

    fn before_run(&self) -> Vec<Node> {
        let mut node0 = Node::new(self.name(), "node0");
        let mut node1 = Node::new(self.name(), "node1");

        node0.modify_app_config(|config: &mut ckb_app_config::CKBAppConfig| {
            config.tx_pool.min_fee_rate = FeeRate::from_u64(0);
        });
        node0.start();

        node1.modify_app_config(|config: &mut ckb_app_config::CKBAppConfig| {
            config.tx_pool.min_fee_rate = FeeRate::from_u64(1_000);
        });
        node1.start();
        vec![node0, node1]
    }
}
