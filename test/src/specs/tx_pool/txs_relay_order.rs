use crate::node::{connect_all, waiting_for_sync};
use crate::util::mining::out_bootstrap_period;
use crate::utils::wait_until;
use crate::{Node, Spec};
use ckb_types::core::FeeRate;
use ckb_types::{
    packed::{CellInput, OutPoint},
    prelude::*,
};

const COUNT: usize = 10;

pub struct TxsRelayOrder;

impl Spec for TxsRelayOrder {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        out_bootstrap_period(nodes);
        connect_all(nodes);
        waiting_for_sync(nodes);

        let node0 = &nodes[0];
        let node1 = &nodes[1];
        // build chain txs
        let mut txs = vec![node0.new_transaction_spend_tip_cellbase()];
        while txs.len() < COUNT {
            let parent = txs.last().unwrap();
            let child = parent
                .as_advanced_builder()
                .set_inputs(vec![{
                    CellInput::new_builder()
                        .previous_output(OutPoint::new(parent.hash(), 0))
                        .build()
                }])
                .set_outputs(vec![parent.output(0).unwrap()])
                .build();
            txs.push(child);
        }
        // submit all txs
        for tx in txs.iter() {
            node0.rpc_client().send_transaction(tx.data().into());
        }
        let tx_pool_info = node0.get_tip_tx_pool_info();
        assert_eq!(COUNT as u64, tx_pool_info.pending.value());
        assert_eq!(0, tx_pool_info.orphan.value());

        // node1 should receive all txs
        let relayed = wait_until(10, || {
            let tx_pool_info = node1.get_tip_tx_pool_info();
            tx_pool_info.pending.value() + tx_pool_info.orphan.value() == COUNT as u64
        });
        let tx_pool_info = node1.get_tip_tx_pool_info();
        assert!(
            relayed,
            "node1 should receive all {COUNT} txs, got pending={}, orphan={}",
            tx_pool_info.pending.value(),
            tx_pool_info.orphan.value()
        );
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_fee_rate = FeeRate::from_u64(0);
    }
}
