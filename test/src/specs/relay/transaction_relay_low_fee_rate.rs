use crate::node::connect_all;
use crate::util::cell::{as_input, as_output, gen_spendable};
use crate::util::check::is_transaction_committed;
use crate::util::log_monitor::monitor_log_until_expected_show;
use crate::util::mining::{out_ibd_mode};
use crate::utils::wait_until;
use crate::{Node, Spec};
use ckb_fee_estimator::FeeRate;
use ckb_types::{core::Capacity, core::TransactionBuilder, packed::CellOutput, prelude::*};
use log::info;
use std::fs;

pub struct TransactionRelayLowFeeRate;

impl Spec for TransactionRelayLowFeeRate {
    crate::setup!(num_nodes: 3);

    fn run(&self, nodes: &mut Vec<Node>) {
        out_ibd_mode(nodes);
        connect_all(nodes);

        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];

        let cells = gen_spendable(node0, 2);
        // As for `low_fee`, which is `inputs.total_capacity == outputs.total_capacity`,
        // so it is a low-fee-rate transaction in this case;
        // As for `high_fee`, it holds minimal output capacity, gives lots of fee.
        let low_fee = TransactionBuilder::default()
            .input(as_input(&cells[0]))
            .output(as_output(&cells[0]))
            .output_data(Default::default())
            .cell_dep(node0.always_success_cell_dep())
            .build();
        let high_fee = TransactionBuilder::default()
            .input(as_input(&cells[1]))
            .output({
                CellOutput::new_builder()
                    .lock(cells[1].cell_output.lock())
                    .type_(cells[1].cell_output.type_())
                    .build_exact_capacity(Capacity::zero())
                    .unwrap()
            })
            .output_data(Default::default())
            .cell_dep(node0.always_success_cell_dep())
            .build();

        let low_cycles = node0
            .rpc_client()
            .dry_run_transaction(low_fee.data().into())
            .cycles;
        let high_cycles = node0
            .rpc_client()
            .dry_run_transaction(high_fee.data().into())
            .cycles;
        node0
            .rpc_client()
            .broadcast_transaction(low_fee.data().into(), low_cycles)
            .unwrap();

        let node0_log_size = fs::metadata(node0.log_path()).unwrap().len();
        let node2_log_size = fs::metadata(node2.log_path()).unwrap().len();

        info!("Broadcast zero fee tx");
        // should only broadcast to node0
        node2.disconnect(node1);
        node1
            .rpc_client()
            .broadcast_transaction(high_fee.data().into(), high_cycles)
            .unwrap();

        let high_relayed = wait_until(10, || is_transaction_committed(node1, &high_fee));
        let low_relayed = wait_until(10, || is_transaction_committed(node1, &low_fee));
        assert!(high_relayed, "high-fee-rate transaction could be relayed");
        assert!(
            !low_relayed,
            "low-fee-rate transaction could not be relayed"
        );
        assert!(monitor_log_until_expected_show(
            node0,
            node0_log_size,
            10,
            "error: SubmitTransaction(The min fee rate is 1000 shannons/KB, so the transaction fee should be 242 shannons at least, but only got 0)"
        )
        .is_some());

        assert!(monitor_log_until_expected_show(
            node2,
            node2_log_size,
            10,
            "received msg RelayTransactions",
        )
        .is_none());
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_fee_rate = FeeRate::from_u64(1_000);
    }
}
