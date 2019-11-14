use crate::specs::tx_pool::utils::assert_new_block_committed;
use crate::{utils::sleep, Net, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_app_config::CKBAppConfig;
use ckb_tx_pool::FeeRate;
use ckb_types::{
    packed::{CellInput, OutPoint},
    prelude::*,
};
use log::debug;

const COUNT: usize = 10;

pub struct SubmitOrphanTx;

impl Spec for SubmitOrphanTx {
    crate::name!("submit_orphan_tx");

    fn run(&self, net: &mut Net) {
        let node0 = &net.nodes[0];

        node0.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);
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

        // send last txs
        let tx = txs.last().unwrap();
        let ret = node0.rpc_client().send_transaction_result(tx.data().into());
        debug!("send_transaction ret {:x} {:?}", tx.hash(), ret);

        // it should be inserted to orphan pool
        let tx_pool_info = node0.rpc_client().tx_pool_info();
        assert_eq!(1, tx_pool_info.orphan.value());

        // submit other txs
        for tx in txs.iter().rev().skip(1) {
            let ret = node0.rpc_client().send_transaction_result(tx.data().into());
            debug!("send_transaction ret {:x} {:?}", tx.hash(), ret);
        }

        node0.generate_block();
        node0.generate_block();
        let tx_pool_info = node0.rpc_client().tx_pool_info();
        assert_eq!(1, tx_pool_info.proposed.value());
        assert_new_block_committed(node0, &[txs[0].clone()]);
        node0.generate_block();
        node0.generate_block();
        let tx_pool_info = node0.rpc_client().tx_pool_info();
        assert_eq!(9, tx_pool_info.proposed.value());
        assert_new_block_committed(node0, &txs[1..10]);
    }

    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig) -> ()> {
        Box::new(|config| {
            config.tx_pool.min_fee_rate = FeeRate::from_u64(0);
        })
    }
}

pub struct RelayChainTx;

impl Spec for RelayChainTx {
    crate::name!("relay_chain_tx");
    crate::setup!(num_nodes: 2);

    fn run(&self, net: &mut Net) {
        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];
        net.exit_ibd_mode();

        node0.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);
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
        assert_eq!(COUNT as u64, tx_pool_info.pending.value() + tx_pool_info.orphan.value());
    }

    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig) -> ()> {
        Box::new(|config| {
            config.tx_pool.min_fee_rate = FeeRate::from_u64(0);
        })
    }
}
