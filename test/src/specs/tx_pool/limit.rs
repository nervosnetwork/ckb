use crate::{assert_regex_match, Net, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
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
            info!("tx.size: {}", tx.serialized_size());
            hash = node.rpc_client().send_transaction(tx.data().into());
            txs_hash.push(hash.clone());
        });

        info!("No.6 tx reach size limit");
        let tx = node.new_transaction(hash.clone());

        let error = node
            .rpc_client()
            .send_transaction_result(tx.data().into())
            .unwrap_err();
        assert_regex_match(&error.to_string(), r"LimitReached");

        // 242 * 5
        node.assert_tx_pool_serialized_size(242 * 5);
        (0..DEFAULT_TX_PROPOSAL_WINDOW.0).for_each(|_| {
            node.generate_block();
        });
        node.generate_block();
        node.assert_tx_pool_serialized_size(0);
    }

    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig) -> ()> {
        Box::new(|config| {
            config.tx_pool.max_mem_size = 242 * 5;
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
            info!("tx.size: {}", tx.serialized_size());
            hash = node.rpc_client().send_transaction(tx.data().into());
            txs_hash.push(hash.clone());
        });

        info!("No.6 tx reach cycles limit");
        let tx = node.new_transaction(hash.clone());

        let error = node
            .rpc_client()
            .send_transaction_result(tx.data().into())
            .unwrap_err();
        assert_regex_match(&error.to_string(), r"LimitReached");

        // 12 * 5
        node.assert_tx_pool_cycles(60);
        (0..DEFAULT_TX_PROPOSAL_WINDOW.0).for_each(|_| {
            node.generate_block();
        });
        node.generate_block();
        node.assert_tx_pool_cycles(0);
    }

    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig) -> ()> {
        Box::new(|config| {
            config.tx_pool.max_mem_size = 20_000_000;
            config.tx_pool.max_cycles = 60;
        })
    }
}
