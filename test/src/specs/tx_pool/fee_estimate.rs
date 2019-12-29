use crate::rpc::RpcClient;
use crate::utils::wait_until;
use crate::{Net, Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_app_config::CKBAppConfig;
use ckb_jsonrpc_types::Status;
use ckb_tx_pool::FeeRate;
use ckb_types::{
    core::{Capacity, TransactionView},
    prelude::*,
};
use log::info;
use rand::{seq::SliceRandom, thread_rng};

const MIN_FEE_RATE: FeeRate = FeeRate::from_u64(1_000);

pub struct FeeEstimate;

fn gen_txs_with_fee(
    node: &Node,
    count: usize,
    min_fee_rate: FeeRate,
    increment_fee_rate: FeeRate,
) -> Vec<(TransactionView, FeeRate)> {
    let mut fee_rate = min_fee_rate;
    let mut txs = Vec::new();
    for _ in 0..count {
        node.generate_block();
        let block = node.get_tip_block();
        let cellbase = &block.transactions()[0];
        let capacity: u64 = cellbase.outputs().get(0).unwrap().capacity().unpack();
        let tx = node.new_transaction_with_since_capacity(
            cellbase.hash(),
            0,
            Capacity::shannons(capacity),
        );
        let fee = fee_rate.fee(tx.data().serialized_size_in_block());
        let output = tx
            .outputs()
            .get(0)
            .unwrap()
            .as_builder()
            .capacity((capacity - fee.as_u64()).pack())
            .build();
        let tx = tx.as_advanced_builder().set_outputs(vec![output]).build();
        txs.push((tx, fee_rate));
        fee_rate = FeeRate::from_u64(fee_rate.as_u64() + increment_fee_rate.as_u64());
    }
    txs
}

fn check_fee_esimate(client: &RpcClient, tx_fee_rates: Vec<FeeRate>) {
    // test wait more blocks should use less fee
    let f1: u64 = client.estimate_fee_rate(3.into()).fee_rate.into();
    let f2: u64 = client.estimate_fee_rate(4.into()).fee_rate.into();
    let f3: u64 = client.estimate_fee_rate(5.into()).fee_rate.into();
    assert!(f1 > f2);
    assert!(f2 > f3);
    // test estimate fee should in a bound
    let min_rate = tx_fee_rates.iter().min().unwrap();
    let max_rate = tx_fee_rates.iter().max().unwrap();
    let mut previous_fee_rate = f1;
    for i in 3..42 {
        let fee_rate: u64 = client.estimate_fee_rate((i as u64).into()).fee_rate.into();
        assert!(
            fee_rate > 0,
            "estimate fee should return a resonable result"
        );
        assert!(
            fee_rate <= previous_fee_rate,
            "should not greater than lesser confirm blocks estimated fee"
        );
        assert!(
            fee_rate > min_rate.as_u64(),
            "estimate fee should greater than min fee rate"
        );
        assert!(
            fee_rate < max_rate.as_u64(),
            "estimate fee should less than max fee rate"
        );
        previous_fee_rate = fee_rate;
    }
}

impl Spec for FeeEstimate {
    crate::name!("fee_estimate");

    fn run(&self, net: &mut Net) {
        // 1. prepare some txs with fee
        // 2. send them then commit them
        // 3. check fee estiamte is between lower bound and upper bound

        let node0 = &net.nodes[0];
        node0.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);

        let min_fee_rate = MIN_FEE_RATE;
        let count = 100;

        let mut txs = gen_txs_with_fee(&node0, count, min_fee_rate, min_fee_rate);

        let mut rng = thread_rng();
        txs.shuffle(&mut rng);

        info!("Send transactions");
        for (tx, _) in &txs {
            node0.rpc_client().send_transaction(tx.data().into());
        }

        // confirm all txs
        let tx_size = txs[0].0.data().serialized_size_in_block();
        for _ in 0..10 {
            // submit 30 txs in each block
            node0.submit_block(&node0.new_block(Some((tx_size * 30) as u64), None, None));
        }

        let ret = wait_until(10, || {
            txs.iter().all(|(tx, _)| {
                node0
                    .rpc_client()
                    .get_transaction(tx.hash())
                    .map(|r| r.tx_status.status == Status::Committed)
                    .unwrap_or(false)
            })
        });

        assert!(ret, "send txs should success");

        info!("Check fee estimate");

        check_fee_esimate(
            node0.rpc_client(),
            txs.iter().map(|(_, fee_rate)| *fee_rate).collect(),
        );
    }

    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig) -> ()> {
        Box::new(|config| {
            config.tx_pool.min_fee_rate = MIN_FEE_RATE;
        })
    }
}
