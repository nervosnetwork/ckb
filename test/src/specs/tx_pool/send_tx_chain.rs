use crate::specs::tx_pool::utils::prepare_tx_family;
use crate::utils::blank;
use crate::utils::propose;
use crate::{Node, Spec};
use ckb_jsonrpc_types::TxStatus;
use ckb_logger::info;
use ckb_types::core::FeeRate;
use ckb_types::{
    packed::{CellInput, OutPoint},
    prelude::*,
};

pub struct SendTxChain;

const MAX_ANCESTORS_COUNT: usize = 2000;
const PROPOSAL_LIMIT: usize = 1500;

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
            .set_proposals(
                txs.iter()
                    .take(PROPOSAL_LIMIT)
                    .map(|tx| tx.proposal_short_id())
                    .collect(),
            )
            .set_transactions(vec![template.transaction(0).unwrap()])
            .build();
        node0.submit_block(&block_with_proposals);
        let block_with_proposals = template
            .as_advanced_builder()
            .set_proposals(
                txs.iter()
                    .skip(PROPOSAL_LIMIT)
                    .map(|tx| tx.proposal_short_id())
                    .collect(),
            )
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

pub struct SendTxChainRevOrder;

impl Spec for SendTxChainRevOrder {
    crate::setup!(num_nodes: 1);

    // Case: Check txpool will evict tx when tx check failed during block_assembler
    //       avoid to stay for a long time in the pool
    fn run(&self, nodes: &mut Vec<Node>) {
        let node_a = &nodes[0];
        let window = node_a.consensus().tx_proposal_window();

        node_a.mine_until_out_bootstrap_period();
        let family = prepare_tx_family(node_a);

        node_a.submit_transaction(family.a());
        node_a.submit_transaction(family.b());

        let tx_pool_info = node_a.rpc_client().tx_pool_info();
        assert_eq!(tx_pool_info.pending.value(), 2);

        // send the child tx firstly
        node_a.submit_block(&propose(node_a, &[family.b()]));

        assert!(node_a.get_transaction(family.a().hash()) == TxStatus::pending());
        assert!(node_a.get_transaction(family.b().hash()) == TxStatus::pending());
        (0..window.closest()).for_each(|_| {
            node_a.submit_block(&blank(node_a));
        });

        assert!(node_a.get_transaction(family.a().hash()) == TxStatus::pending());

        // tx_b is removed by txpool, don't stay in the pool
        assert!(node_a.get_transaction(family.b().hash()) == TxStatus::unknown());
        let tx_pool_info = node_a.rpc_client().tx_pool_info();
        assert_eq!(tx_pool_info.pending.value(), 1);
    }
}
