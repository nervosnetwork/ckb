use crate::{Node, Spec};

use ckb_logger::info;
use ckb_types::core::FeeRate;
use std::{thread::sleep, time::Duration};

pub struct SizeLimit;

const MAX_MEM_SIZE_FOR_SIZE_LIMIT: usize = 2000;

impl Spec for SizeLimit {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];

        info!("Generate DEFAULT_TX_PROPOSAL_WINDOW block on node");
        node.mine_until_out_bootstrap_period();

        info!("Generate 1 tx on node");
        let mut txs_hash = Vec::new();
        let tx = node.new_transaction_spend_tip_cellbase();
        let mut hash = node.submit_transaction(&tx);
        txs_hash.push(hash.clone());

        let tx_pool_info = node.get_tip_tx_pool_info();
        let one_tx_size = tx_pool_info.total_tx_size.value();
        let one_tx_cycles = tx_pool_info.total_tx_cycles.value();

        info!(
            "one_tx_cycles: {}, one_tx_size: {}",
            one_tx_cycles, one_tx_size
        );

        assert!(MAX_MEM_SIZE_FOR_SIZE_LIMIT as u64 > one_tx_size * 2);

        let max_tx_num = (MAX_MEM_SIZE_FOR_SIZE_LIMIT as u64) / one_tx_size;

        info!("Generate as much as possible txs on node");
        (0..(max_tx_num - 1)).for_each(|_| {
            let tx = node.new_transaction(hash.clone());
            hash = node.rpc_client().send_transaction(tx.data().into());
            txs_hash.push(hash.clone());
            sleep(Duration::from_millis(10));
        });

        info!("The next tx reach size limit");
        let tx = node.new_transaction(hash);
        let _hash = node.rpc_client().send_transaction(tx.data().into());
        node.assert_tx_pool_serialized_size((max_tx_num + 1) * one_tx_size);
        let last = node
            .mine_with_blocking(|template| template.proposals.len() != (max_tx_num + 1) as usize);
        node.assert_tx_pool_serialized_size(max_tx_num * one_tx_size);
        node.mine_with_blocking(|template| template.number.value() != (last + 1));
        node.mine_with_blocking(|template| template.transactions.len() != max_tx_num as usize);
        node.assert_tx_pool_serialized_size(0);
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.max_mem_size = MAX_MEM_SIZE_FOR_SIZE_LIMIT;
        config.tx_pool.min_fee_rate = FeeRate::zero();
    }
}
