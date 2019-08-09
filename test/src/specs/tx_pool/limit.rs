use crate::utils::assert_send_transaction_fail;
use crate::{Net, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_app_config::CKBAppConfig;
use log::info;

pub struct SizeLimit;

impl Spec for SizeLimit {
    crate::name!("size_limit");

    fn run(&self, net: Net) {
        let node = &net.nodes[0];

        info!("Generate 1 block on node");
        node.generate_block();

        info!("Generate 5 txs on node");
        let mut txs_hash = Vec::new();
        let mut hash = node.generate_transaction();
        txs_hash.push(hash.clone());

        (0..4).for_each(|_| {
            let tx = node.new_transaction(hash.clone());
            hash = node.rpc_client().send_transaction((&tx).into());
            txs_hash.push(hash.clone());
        });

        info!("No.6 tx reach size limit");
        let tx = node.new_transaction(hash.clone());
        assert_send_transaction_fail(node, &tx, "Full Transaction Pool");

        // 181 * 5
        // 12 * 5
        node.assert_tx_pool_statics(905, 60);
        (0..DEFAULT_TX_PROPOSAL_WINDOW.0).for_each(|_| {
            node.generate_block();
        });
        node.generate_block();
        node.assert_tx_pool_statics(0, 0);
    }

    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig) -> ()> {
        Box::new(|config| {
            config.tx_pool.max_mem_size = 905;
            config.tx_pool.max_cycles = 200_000_000_000;
        })
    }
}

pub struct CyclesLimit;

impl Spec for CyclesLimit {
    crate::name!("cycles_limit");

    fn run(&self, net: Net) {
        let node = &net.nodes[0];

        info!("Generate 1 block on node");
        node.generate_block();

        info!("Generate 5 txs on node");
        let mut txs_hash = Vec::new();
        let mut hash = node.generate_transaction();
        txs_hash.push(hash.clone());

        (0..4).for_each(|_| {
            let tx = node.new_transaction(hash.clone());
            hash = node.rpc_client().send_transaction((&tx).into());
            txs_hash.push(hash.clone());
        });

        info!("No.6 tx reach cycles limit");
        let tx = node.new_transaction(hash.clone());
        assert_send_transaction_fail(node, &tx, "Full Transaction Pool");

        // 181 * 5
        // 12 * 5
        node.assert_tx_pool_statics(905, 60);
        (0..DEFAULT_TX_PROPOSAL_WINDOW.0).for_each(|_| {
            node.generate_block();
        });
        node.generate_block();
        node.assert_tx_pool_statics(0, 0);
    }

    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig) -> ()> {
        Box::new(|config| {
            config.tx_pool.max_mem_size = 20_000_000;
            config.tx_pool.max_cycles = 60;
        })
    }
}
