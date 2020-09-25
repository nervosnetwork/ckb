use crate::node::{connect_all, exit_ibd_mode};
use crate::utils::sleep;
use crate::{Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_fee_estimator::FeeRate;
use ckb_types::{
    packed::{CellInput, OutPoint},
    prelude::*,
};

const COUNT: usize = 10;

pub struct TxsRelayOrder;

impl Spec for TxsRelayOrder {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        connect_all(nodes);
        exit_ibd_mode(nodes);

        let node0 = &nodes[0];
        let node1 = &nodes[1];

        node0.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);
        node1.waiting_for_sync(node0, node0.get_tip_block().header().number());
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
        sleep(10);
        let tx_pool_info = node1.get_tip_tx_pool_info();
        assert_eq!(
            COUNT as u64,
            tx_pool_info.pending.value() + tx_pool_info.orphan.value()
        );
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_fee_rate = FeeRate::from_u64(0);
    }
}
