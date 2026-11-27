use crate::node::{connect_all, wait_relay_protocols_connected, waiting_for_sync};
use crate::util::cell::{as_input, as_output, gen_spendable};
use crate::util::log_monitor::monitor_log_until_expected_show;
use crate::util::mining::out_ibd_mode;
use crate::{Node, Spec};
use ckb_logger::debug;
use ckb_types::core::{FeeRate, TransactionBuilder};
use ckb_types::packed::Bytes;

pub struct TransactionRelayLowFeeRate;

impl Spec for TransactionRelayLowFeeRate {
    fn run(&self, nodes: &mut Vec<Node>) {
        out_ibd_mode(nodes);

        let node0 = &nodes[0];
        let node1 = &nodes[1];

        let cells = gen_spendable(node0, 1);
        // As for `low_fee`, which is `inputs.total_capacity == outputs.total_capacity`,
        // so it is a low-fee-rate transaction in this case;
        let low_fee = TransactionBuilder::default()
            .input(as_input(&cells[0]))
            .output(as_output(&cells[0]))
            .output_data(Bytes::default())
            .cell_dep(node0.always_success_cell_dep())
            .build();

        debug!("make sure node1 has the cell");
        connect_all(nodes);
        waiting_for_sync(nodes);
        wait_relay_protocols_connected(nodes);

        node0.rpc_client().send_transaction(low_fee.data().into());

        assert!(monitor_log_until_expected_show(
            node1,
            0,
            10,
            "Reject tx The min fee rate is 1000 shannons/KW, requiring a transaction fee of at least 242 shannons, but the fee provided is only 0"
        )
        .is_some());
    }

    fn before_run(&self) -> Vec<Node> {
        let node0 = Node::new_with_modifiers(
            self.name(),
            "node0",
            |config| {
                config.tx_pool.min_fee_rate = FeeRate::from_u64(0);
            },
            |_| {},
        );
        let node1 = Node::new_with_modifiers(
            self.name(),
            "node1",
            |config| {
                config.tx_pool.min_fee_rate = FeeRate::from_u64(1_000);
            },
            |_| {},
        );

        let mut nodes = vec![node0, node1];
        crate::utils::run_in_spec_scope(nodes.iter_mut(), |node| node.start());
        nodes
    }
}
