use crate::node::waiting_for_sync;
use crate::node::{connect_all, disconnect_all};
use crate::util::check::is_transaction_proposed;
use crate::util::mining::out_ibd_mode;
use crate::{Node, Spec};
use ckb_jsonrpc_types::ProposalShortId;
use ckb_logger::info;
use ckb_types::core::{capacity_bytes, Capacity, FeeRate};
use ckb_types::packed::CellOutputBuilder;
use ckb_types::{
    packed::{self, CellInput, OutPoint},
    prelude::*,
};

pub struct PoolReconcile;

impl Spec for PoolReconcile {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];

        info!("Generate DEFAULT_TX_PROPOSAL_WINDOW block on node0");
        node0.mine_until_out_bootstrap_period();

        info!("Use generated block's cellbase as tx input");
        let hash = node0.generate_transaction();

        info!("Generate 3 more blocks on node0");
        node0.mine_until_transaction_confirm(&hash);

        info!("Pool should be empty");
        assert!(node0
            .rpc_client()
            .get_transaction(hash.clone())
            .tx_status
            .block_hash
            .is_some());

        info!("Generate 5 blocks on node1");
        node1.mine(20);

        info!("Connect node0 to node1");
        node0.connect(node1);

        waiting_for_sync(nodes);

        info!("Tx should be re-added to node0's pool");
        assert!(node0
            .rpc_client()
            .get_transaction(hash)
            .tx_status
            .block_hash
            .is_none());
    }
}

pub struct PoolResolveConflictAfterReorg;

impl Spec for PoolResolveConflictAfterReorg {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        out_ibd_mode(nodes);
        connect_all(nodes);

        let node0 = &nodes[0];
        let node1 = &nodes[1];

        node0.mine_until_out_bootstrap_period();
        waiting_for_sync(nodes);

        info!("Use generated block's cellbase as tx input");
        let tx1 = node0.new_transaction_spend_tip_cellbase();
        // build txs chain
        let mut txs = vec![tx1.clone()];
        while txs.len() < 3 {
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
        assert_eq!(txs.len(), 3);

        info!("Disconnect nodes");
        disconnect_all(nodes);

        info!("submit tx1 chain to node0");
        let ret = node0
            .rpc_client()
            .send_transaction_result(tx1.data().into());
        assert!(ret.is_ok());

        let target: ProposalShortId = packed::ProposalShortId::from_tx_hash(&tx1.hash()).into();
        let last =
            node0.mine_with_blocking(|template| !template.proposals.iter().any(|id| id == &target));
        node0.mine_with_blocking(|template| template.number.value() != (last + 1));
        for tx in txs[1..].iter() {
            let ret = node0.rpc_client().send_transaction_result(tx.data().into());
            assert!(ret.is_ok());
        }
        let block = node0
            .new_block_builder_with_blocking(|template| {
                !(template
                    .transactions
                    .iter()
                    .any(|tx| tx.hash == tx1.hash().unpack()))
            })
            .set_proposals(txs.iter().map(|tx| tx.proposal_short_id()).collect())
            .build();
        node0.submit_block(&block);

        node0.mine_with_blocking(|template| template.number.value() != (block.number() + 1));
        node0.wait_for_tx_pool();

        for tx in txs[1..].iter() {
            assert!(is_transaction_proposed(node0, tx));
        }

        info!("Tx1 mined by node1");
        assert!(node0
            .rpc_client()
            .get_transaction(tx1.hash())
            .tx_status
            .block_hash
            .is_some());

        let tip_number0 = node0.get_tip_block_number();
        info!("Mine until node1 > node0");
        while node1.get_tip_block_number() < tip_number0 + 1 {
            let proposed_block = node1
                .new_block_builder(None, None, None)
                .set_proposals(txs.iter().map(|tx| tx.proposal_short_id()).collect())
                .build();
            node1.submit_block(&proposed_block);
        }

        info!("Connect node0 to node1");
        node0.connect(node1);

        waiting_for_sync(nodes);
        node0.wait_for_tx_pool();

        for tx in txs.iter() {
            assert!(is_transaction_proposed(node0, tx));
        }

        let conflict_tx = tx1
            .as_advanced_builder()
            .set_inputs(vec![{
                CellInput::new_builder()
                    .previous_output(OutPoint::new(tx1.hash(), 0))
                    .build()
            }])
            .set_outputs(vec![CellOutputBuilder::default()
                .capacity(capacity_bytes!(99).pack())
                .build()])
            .build();

        let ret = node0
            .rpc_client()
            .send_transaction_result(conflict_tx.data().into());
        assert!(ret.is_err());
        let err_msg = ret.err().unwrap().to_string();
        assert!(err_msg.contains("Resolve failed Dead"));
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_fee_rate = FeeRate::from_u64(0);
    }
}
