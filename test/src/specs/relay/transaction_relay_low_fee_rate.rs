use crate::node::{connect_all, exit_ibd_mode};
use crate::util::check::is_transaction_committed;
use crate::util::log_monitor::monitor_log_until_expected_show;
use crate::utils::wait_until;
use crate::{Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_fee_estimator::FeeRate;
use ckb_types::{core::TransactionView, packed, prelude::*};
use log::info;
use std::fs;

pub struct TransactionRelayLowFeeRate;

impl Spec for TransactionRelayLowFeeRate {
    crate::setup!(num_nodes: 3);

    fn run(&self, nodes: &mut Vec<Node>) {
        exit_ibd_mode(nodes);
        connect_all(nodes);

        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];

        info!("Generate new transaction on node1");
        node1.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);
        let tx = node1.new_transaction_spend_tip_cellbase();
        node1.submit_transaction(&tx);
        let hash = tx.hash();
        // confirm tx
        node1.generate_blocks(20);
        let ret = wait_until(10, || is_transaction_committed(node1, &tx));
        assert!(ret, "send tx should success");
        let tx: TransactionView = packed::Transaction::from(
            node1
                .rpc_client()
                .get_transaction(hash.clone())
                .unwrap()
                .transaction
                .inner,
        )
        .into_view();
        let capacity = tx.outputs_capacity().unwrap();

        info!("Generate zero fee rate tx");
        let tx_low_fee = node1.new_transaction(hash);
        // Set to zero fee
        let output = tx_low_fee
            .outputs()
            .get(0)
            .unwrap()
            .as_builder()
            .capacity(capacity.pack())
            .build();
        let tx_low_fee = tx_low_fee
            .data()
            .as_advanced_builder()
            .set_outputs(vec![])
            .output(output)
            .build();

        info!("Get tx cycles");
        let cycles = node1
            .rpc_client()
            .dry_run_transaction(tx_low_fee.data().into())
            .cycles;

        let node0_log_size = fs::metadata(node0.log_path()).unwrap().len();
        let node2_log_size = fs::metadata(node2.log_path()).unwrap().len();

        info!("Broadcast zero fee tx");
        // should only broadcast to node0
        node2.disconnect(node1);
        node1
            .rpc_client()
            .broadcast_transaction(tx_low_fee.data().into(), cycles)
            .unwrap();

        assert!(monitor_log_until_expected_show(
            node0,
            node0_log_size,
            10,
            "error: SubmitTransaction(Transaction fee rate must >= 242 shannons/KB, got: 0)"
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
