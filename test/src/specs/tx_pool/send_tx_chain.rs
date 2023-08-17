use crate::{Node, Spec};
use ckb_logger::info;
use ckb_types::core::FeeRate;
use ckb_types::{
    packed::{CellInput, OutPoint},
    prelude::*,
};

pub struct SendTxChain;

const MAX_ANCESTORS_COUNT: usize = 125;

impl Spec for SendTxChain {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();
        // build txs chain
        let mut txs = vec![node0.new_transaction_spend_tip_cellbase()];
        while txs.len() < MAX_ANCESTORS_COUNT + 1 {
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
        assert_eq!(txs.len(), MAX_ANCESTORS_COUNT + 1);
        // send tx chain
        info!("submit fresh txs chain to node0");
        for tx in txs[..=MAX_ANCESTORS_COUNT - 1].iter() {
            let ret = node0.rpc_client().send_transaction_result(tx.data().into());
            assert!(ret.is_ok());
        }
        // The last one will be rejected
        let ret = node0
            .rpc_client()
            .send_transaction_result(txs[MAX_ANCESTORS_COUNT].data().into());
        assert!(ret.is_err());

        node0.mine(3);

        // build txs chain
        let mut txs = vec![node0.new_transaction_spend_tip_cellbase()];
        while txs.len() < MAX_ANCESTORS_COUNT + 1 {
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
        assert_eq!(txs.len(), MAX_ANCESTORS_COUNT + 1);

        let template = node0.new_block(None, None, None);
        let block_with_proposals = template
            .as_advanced_builder()
            .set_proposals(txs.iter().map(|tx| tx.proposal_short_id()).collect())
            .set_transactions(vec![template.transaction(0).unwrap()])
            .build();
        node0.submit_block(&block_with_proposals);
        node0.mine(node0.consensus().tx_proposal_window().closest());

        info!("submit proposed txs chain to node0");
        // send tx chain
        for tx in txs[..MAX_ANCESTORS_COUNT].iter() {
            let ret = node0.rpc_client().send_transaction_result(tx.data().into());
            assert!(ret.is_ok());
        }
        let ret = node0
            .rpc_client()
            .send_transaction_result(txs.last().unwrap().data().into());
        assert!(ret.is_err());
        assert!(ret
            .err()
            .unwrap()
            .to_string()
            .contains("Transaction exceeded maximum ancestors count limit"));
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_fee_rate = FeeRate::from_u64(0);
        config.tx_pool.max_ancestors_count = MAX_ANCESTORS_COUNT;
    }
}
