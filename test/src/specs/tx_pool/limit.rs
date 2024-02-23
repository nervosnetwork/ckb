use crate::{
    util::{cell::gen_spendable, transaction::always_success_transaction},
    Node, Spec,
};
use ckb_logger::info;
use ckb_types::{
    core::{cell::CellMetaBuilder, DepType, FeeRate},
    packed::CellDepBuilder,
};
use std::{thread::sleep, time::Duration};

use ckb_types::{packed::OutPoint, prelude::*};

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

        info!("Generate as much as possible txs on : {}", max_tx_num);
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
        config.tx_pool.max_tx_pool_size = MAX_MEM_SIZE_FOR_SIZE_LIMIT;
        config.tx_pool.min_fee_rate = FeeRate::zero();
    }
}

pub struct TxPoolLimitAncestorCount;
impl Spec for TxPoolLimitAncestorCount {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        let initial_inputs = gen_spendable(node0, 130);
        let input_a = &initial_inputs[0];

        // Commit transaction root
        let tx_a = {
            let tx_a = always_success_transaction(node0, input_a);
            node0.submit_transaction(&tx_a);
            tx_a
        };

        let cell_dep = CellDepBuilder::default()
            .dep_type(DepType::Code.into())
            .out_point(OutPoint::new(tx_a.hash(), 0))
            .build();

        // Create 125 transactions cell dep on tx_a
        for i in 1..=125 {
            let cur = always_success_transaction(node0, initial_inputs.get(i).unwrap());
            let cur = cur.as_advanced_builder().cell_dep(cell_dep.clone()).build();
            let _ = node0.rpc_client().send_transaction(cur.data().into());
        }

        // Create a new transaction consume the cell dep, it will be succeed in submit
        let input = CellMetaBuilder::from_cell_output(tx_a.output(0).unwrap(), Default::default())
            .out_point(OutPoint::new(tx_a.hash(), 0))
            .build();
        let last = always_success_transaction(node0, &input);

        let res = node0
            .rpc_client()
            .send_transaction_result(last.data().into());
        assert!(res.is_ok());

        // create a transaction chain
        let input_c = &initial_inputs[129];
        // Commit transaction root
        let tx_c = {
            let tx_c = always_success_transaction(node0, input_c);
            node0.submit_transaction(&tx_c);
            tx_c
        };

        let mut prev = tx_c.clone();
        // Create transaction chain
        for i in 0..125 {
            let input =
                CellMetaBuilder::from_cell_output(prev.output(0).unwrap(), Default::default())
                    .out_point(OutPoint::new(prev.hash(), 0))
                    .build();
            let cur = always_success_transaction(node0, &input);
            let res = node0
                .rpc_client()
                .send_transaction_result(cur.data().into());
            prev = cur.clone();
            if i >= 124 {
                assert!(res.is_err());
                let msg = res.err().unwrap().to_string();
                assert!(msg.contains("PoolRejectedTransactionByMaxAncestorsCountLimit"));
            } else {
                assert!(res.is_ok());
            }
        }
    }
}
