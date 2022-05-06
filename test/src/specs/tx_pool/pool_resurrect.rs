use crate::node::waiting_for_sync;
use crate::{Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_logger::info;

pub struct PoolResurrect;

impl Spec for PoolResurrect {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];

        info!("Generate 1 block on node0");
        node0.mine_until_out_bootstrap_period();

        info!("Generate 6 txs on node0");
        let mut txs_hash = Vec::new();
        let mut hash = node0.generate_transaction();
        txs_hash.push(hash.clone());

        (0..5).for_each(|_| {
            let tx = node0.new_transaction(hash.clone());
            hash = node0.rpc_client().send_transaction(tx.data().into());
            txs_hash.push(hash.clone());
        });

        info!("Generate 3 more blocks on node0");
        let proposed =
            node0.mine_with_blocking(|template| template.proposals.len() != txs_hash.len());
        node0.mine_with_blocking(|template| template.number.value() != (proposed + 1));
        node0.mine_with_blocking(|template| template.transactions.len() != txs_hash.len());
        node0.wait_for_tx_pool();

        info!("Pool should be empty");
        node0.assert_tx_pool_size(0, 0);

        info!("Generate 5 blocks on node1");
        node1.mine(DEFAULT_TX_PROPOSAL_WINDOW.1 + 6);

        info!("Connect node0 to node1, waiting for sync");
        node0.connect(node1);
        waiting_for_sync(nodes);

        info!("6 txs should be returned to node0 pending pool");
        node0.assert_tx_pool_size(txs_hash.len() as u64, 0);

        info!("Generate 2 blocks on node0, 6 txs should be added to proposed pool");
        let proposed = node0.mine_with_blocking(|template| {
            template.uncles.clear();
            template.proposals.len() != txs_hash.len()
        });
        node0.mine_with_blocking(|template| {
            template.uncles.clear();
            template.number.value() != (proposed + 1)
        });

        info!("Generate 1 block on node0, 6 txs should be included in this block");
        node0.mine_with_blocking(|template| template.transactions.len() != txs_hash.len());
        node0.wait_for_tx_pool();
        node0.assert_tx_pool_size(0, 0);
    }
}

pub struct InvalidHeaderDep;

impl Spec for InvalidHeaderDep {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];

        info!("Generate 1 block on node0");
        node0.mine_until_out_bootstrap_period();
        node0.mine(1);

        info!("Generate header dep tx on node0");
        let hash = node0.generate_transaction();

        let tip = node0.get_tip_block();

        let tx = node0.new_transaction(hash);

        node0.rpc_client().send_transaction(
            tx.as_advanced_builder()
                .set_header_deps(vec![tip.hash()])
                .build()
                .data()
                .into(),
        );

        let tx_pool_info = node0.get_tip_tx_pool_info();
        assert_eq!(tx_pool_info.pending.value(), 2);

        node1.mine_until_out_bootstrap_period();
        node1.mine(2);

        info!("Connect node0 to node1, waiting for sync");
        node0.connect(node1);
        waiting_for_sync(nodes);

        node0.wait_for_tx_pool();

        info!("invalid header dep tx should be removed");
        node0.assert_tx_pool_size(1, 0);
    }
}
