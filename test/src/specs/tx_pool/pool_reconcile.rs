use crate::{Net, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use log::info;

pub struct PoolReconcile;

impl Spec for PoolReconcile {
    crate::name!("pool_reconcile");

    crate::setup!(connect_all: false, num_nodes: 2);

    fn run(&self, net: &mut Net) {
        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];

        info!("Generate DEFAULT_TX_PROPOSAL_WINDOW block on node0");
        node0.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);

        info!("Use generated block's cellbase as tx input");
        let hash = node0.generate_transaction();

        info!("Generate 3 more blocks on node0");
        node0.generate_blocks(3);

        info!("Pool should be empty");
        assert!(node0
            .rpc_client()
            .get_transaction(hash.clone())
            .unwrap()
            .tx_status
            .block_hash
            .is_some());

        info!("Generate 5 blocks on node1");
        node1.generate_blocks(20);

        info!("Connect node0 to node1");
        node0.connect(node1);

        net.waiting_for_sync(20);

        info!("Tx should be re-added to node0's pool");
        assert!(node0
            .rpc_client()
            .get_transaction(hash)
            .unwrap()
            .tx_status
            .block_hash
            .is_none());
    }
}
