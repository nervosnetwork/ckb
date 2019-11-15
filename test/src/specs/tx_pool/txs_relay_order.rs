use crate::{utils::sleep, Net, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_app_config::CKBAppConfig;
use ckb_tx_pool::FeeRate;
use ckb_types::{
    packed::{CellInput, OutPoint},
    prelude::*,
};

const COUNT: usize = 10;

pub struct TxsRelayOrder;

impl Spec for TxsRelayOrder {
    crate::name!("txs_relay_order");
    crate::setup!(num_nodes: 2);

    fn run(&self, net: &mut Net) {
        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];
        net.exit_ibd_mode();

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
        let tx_pool_info = node0.rpc_client().tx_pool_info();
        assert_eq!(COUNT as u64, tx_pool_info.pending.value());
        assert_eq!(0, tx_pool_info.orphan.value());

        // node1 should receive all txs
        sleep(10);
        let tx_pool_info = node1.rpc_client().tx_pool_info();
        assert_eq!(
            COUNT as u64,
            tx_pool_info.pending.value() + tx_pool_info.orphan.value()
        );
    }

    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig) -> ()> {
        Box::new(|config| {
            config.tx_pool.min_fee_rate = FeeRate::from_u64(0);
        })
    }
}
