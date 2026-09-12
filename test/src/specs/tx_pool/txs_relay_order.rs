use super::utils::{get_pool_entries, wait_for_pending_count};
use crate::node::{connect_all, waiting_for_sync};
use crate::util::mining::out_ibd_mode;
use crate::{Node, Spec};
use ckb_types::core::FeeRate;
use ckb_types::{
    packed::{CellInput, OutPoint},
    prelude::*,
};

const COUNT: usize = 10;

pub struct TxsRelayOrder;

impl Spec for TxsRelayOrder {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        out_ibd_mode(nodes);
        connect_all(nodes);

        let node0 = &nodes[0];
        let node1 = &nodes[1];

        node0.mine_until_out_bootstrap_period();
        waiting_for_sync(nodes);
        // build chain txs
        let mut txs = vec![node0.new_transaction_spend_tip_cellbase()];
        while txs.len() < COUNT {
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
        // submit all txs
        for tx in txs.iter() {
            node0.rpc_client().send_transaction(tx.data().into());
        }
        let tx_pool_info = node0.get_tip_tx_pool_info();
        assert_eq!(COUNT as u64, tx_pool_info.pending.value());
        assert_eq!(0, tx_pool_info.orphan.value());

        // Receiving orphan bodies is not completion: every dependent must
        // resolve and verify after its parent, leaving the exact accepted set.
        wait_for_pending_count(node1, COUNT as u64);
        let tx_pool_info = node1.get_tip_tx_pool_info();
        assert_eq!(tx_pool_info.orphan.value(), 0);
        assert_eq!(tx_pool_info.verify_queue_size.value(), 0);
        let entries = get_pool_entries(node1);
        assert!(entries.proposed.is_empty());
        assert_eq!(entries.pending.len(), COUNT);
        for (index, tx) in txs.iter().enumerate() {
            let hash = tx.hash().into();
            let entry = entries
                .pending
                .get(&hash)
                .expect("relayed transaction is accepted");
            assert_eq!(entry.ancestors_count.value(), index as u64 + 1);
            assert_eq!(
                node1.get_transaction(tx.hash()),
                ckb_jsonrpc_types::TxStatus::pending()
            );
        }
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_fee_rate = FeeRate::from_u64(0);
    }
}
