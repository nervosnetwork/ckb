use crate::utils::wait_until;
use crate::{Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};

use ckb_fee_estimator::FeeRate;
use ckb_types::{core::TransactionView, packed, prelude::*};
use log::info;

pub struct SendLowFeeRateTx;

impl Spec for SendLowFeeRateTx {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);
        let tx_hash_0 = node0.generate_transaction();
        let ret = wait_until(10, || {
            node0
                .rpc_client()
                .get_transaction(tx_hash_0.clone())
                .is_some()
        });
        assert!(ret, "send tx should success");
        let tx: TransactionView = packed::Transaction::from(
            node0
                .rpc_client()
                .get_transaction(tx_hash_0.clone())
                .unwrap()
                .transaction
                .inner,
        )
        .into_view();
        let capacity = tx.outputs_capacity().unwrap();

        info!("Generate zero fee rate tx");
        let tx_low_fee = node0.new_transaction(tx_hash_0.clone());
        // Set to zero fee
        let output = tx_low_fee
            .outputs()
            .get(0)
            .unwrap()
            .as_builder()
            .capacity(capacity.pack())
            .build();
        let tx_low_fee = tx_low_fee
            .data()
            .as_advanced_builder()
            .set_outputs(vec![])
            .output(output)
            .build();
        let ret = node0
            .rpc_client()
            .send_transaction_result(tx_low_fee.data().into());
        assert!(ret.is_err());

        info!("Generate normal fee rate tx");
        let tx_high_fee = node0.new_transaction(tx_hash_0);
        let output = tx_high_fee
            .outputs()
            .get(0)
            .unwrap()
            .as_builder()
            .capacity(capacity.safe_sub(1000u32).unwrap().pack())
            .build();
        let tx_high_fee = tx_high_fee
            .data()
            .as_advanced_builder()
            .set_outputs(vec![])
            .output(output)
            .build();
        node0
            .rpc_client()
            .send_transaction(tx_high_fee.data().into());
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_fee_rate = FeeRate::from_u64(1_000);
    }
}
