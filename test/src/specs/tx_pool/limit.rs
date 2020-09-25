use crate::utils::assert_send_transaction_fail;
use crate::{Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};

use ckb_fee_estimator::FeeRate;
use log::info;

pub struct SizeLimit;

const MAX_CYCLES_FOR_SIZE_LIMIT: u64 = 200_000_000_000;
const MAX_MEM_SIZE_FOR_SIZE_LIMIT: usize = 2000;

impl Spec for SizeLimit {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];

        info!("Generate DEFAULT_TX_PROPOSAL_WINDOW block on node");
        node.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);

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

        assert!(one_tx_cycles * max_tx_num < MAX_CYCLES_FOR_SIZE_LIMIT);

        info!("Generate as much as possible txs on node");
        (0..(max_tx_num - 1)).for_each(|_| {
            let tx = node.new_transaction(hash.clone());
            hash = node.rpc_client().send_transaction(tx.data().into());
            txs_hash.push(hash.clone());
        });

        info!("The next tx reach size limit");
        let tx = node.new_transaction(hash);
        assert_send_transaction_fail(node, &tx, "Transaction pool exceeded maximum size limit");

        node.assert_tx_pool_serialized_size(max_tx_num * one_tx_size);
        (0..DEFAULT_TX_PROPOSAL_WINDOW.0).for_each(|_| {
            node.generate_block();
        });
        node.generate_block();
        node.assert_tx_pool_serialized_size(0);
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.max_mem_size = MAX_MEM_SIZE_FOR_SIZE_LIMIT;
        config.tx_pool.max_cycles = MAX_CYCLES_FOR_SIZE_LIMIT;
        config.tx_pool.min_fee_rate = FeeRate::zero();
    }
}

pub struct CyclesLimit;

const MAX_CYCLES_FOR_CYCLE_LIMIT: u64 = 6000;
const MAX_MEM_SIZE_FOR_CYCLE_LIMIT: usize = 20_000_000;

impl Spec for CyclesLimit {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];

        info!("Generate DEFAULT_TX_PROPOSAL_WINDOW block on node");
        node.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);

        info!("Generate 1 tx on node");
        let mut txs_hash = Vec::new();
        let tx = node.new_transaction_spend_tip_cellbase();
        let mut hash = node.submit_transaction(&tx);
        txs_hash.push(hash.clone());

        let tx_pool_info = node.get_tip_tx_pool_info();
        let one_tx_cycles = tx_pool_info.total_tx_cycles.value();
        let one_tx_size = tx.data().serialized_size_in_block();

        info!(
            "one_tx_cycles: {}, one_tx_size: {}",
            one_tx_cycles, one_tx_size
        );

        assert!(MAX_CYCLES_FOR_CYCLE_LIMIT > one_tx_cycles * 2);

        let max_tx_num = MAX_CYCLES_FOR_CYCLE_LIMIT / one_tx_cycles;

        assert!(one_tx_size * (max_tx_num as usize) < MAX_MEM_SIZE_FOR_CYCLE_LIMIT);

        info!("Generate as much as possible txs on node");
        (0..(max_tx_num - 1)).for_each(|_| {
            let tx = node.new_transaction(hash.clone());
            hash = node.rpc_client().send_transaction(tx.data().into());
            txs_hash.push(hash.clone());
        });

        info!("The next tx reach cycles limit");
        let tx = node.new_transaction(hash);
        assert_send_transaction_fail(node, &tx, "Transaction pool exceeded maximum cycles limit");

        node.assert_tx_pool_cycles(max_tx_num * one_tx_cycles);
        (0..DEFAULT_TX_PROPOSAL_WINDOW.0).for_each(|_| {
            node.generate_block();
        });
        node.generate_block();
        node.assert_tx_pool_cycles(0);
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.max_mem_size = MAX_MEM_SIZE_FOR_CYCLE_LIMIT;
        config.tx_pool.max_cycles = MAX_CYCLES_FOR_CYCLE_LIMIT;
        config.tx_pool.min_fee_rate = FeeRate::zero();
    }
}
