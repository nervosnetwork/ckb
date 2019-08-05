use crate::utils::wait_until;
use crate::{assert_regex_match, Net, Spec};
use ckb_app_config::CKBAppConfig;
use ckb_core::transaction::{Transaction, TransactionBuilder};
use ckb_core::Capacity;
use ckb_shared::fee_rate::FeeRate;
use log::info;

pub struct SendLowFeeRateTx;

impl Spec for SendLowFeeRateTx {
    crate::name!("send_low_fee_rate_tx");

    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];

        node0.generate_block();
        let tx_hash_0 = node0.generate_transaction();
        let ret = wait_until(10, || {
            node0
                .rpc_client()
                .get_transaction(tx_hash_0.clone())
                .is_some()
        });
        assert!(ret, "send tx should success");
        let tx: Transaction = node0
            .rpc_client()
            .get_transaction(tx_hash_0.clone())
            .unwrap()
            .transaction
            .inner
            .into();
        let capacity = tx.outputs_capacity().unwrap();

        info!("Generate zero fee rate tx");
        let tx_low_fee = node0.new_transaction(tx_hash_0.clone());
        // Set to zero fee
        let mut output = tx_low_fee.outputs()[0].clone();
        output.capacity = capacity;
        let tx_low_fee = TransactionBuilder::from_transaction(tx_low_fee)
            .outputs_clear()
            .output(output)
            .build();
        let error = node0
            .rpc_client()
            .send_transaction_result((&tx_low_fee).into())
            .unwrap_err();
        assert_regex_match(&error.to_string(), r"TxFeeToLow");

        info!("Generate normal fee rate tx");
        let tx_high_fee = node0.new_transaction(tx_hash_0.clone());
        let mut output = tx_high_fee.outputs()[0].clone();
        output.capacity = capacity.safe_sub(1000u32).unwrap();
        let tx_high_fee = TransactionBuilder::from_transaction(tx_high_fee)
            .outputs_clear()
            .output(output)
            .build();
        node0.rpc_client().send_transaction((&tx_high_fee).into());
    }

    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig) -> ()> {
        Box::new(|config| {
            config.tx_pool.min_fee_rate = FeeRate::new(Capacity::shannons(1000));
        })
    }
}
