use crate::{Net, Spec};
use log::info;

pub struct PoolReconcile;

impl Spec for PoolReconcile {
    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];

        info!("Generate 1 block on node0");
        node0.generate_block();

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
        node1.generate_blocks(5);

        info!("Connect node0 to node1");
        node0.connect(node1);

        net.waiting_for_sync(5);

        info!("Tx should be re-added to node0's pool");
        assert!(node0
            .rpc_client()
            .get_transaction(hash.clone())
            .unwrap()
            .tx_status
            .block_hash
            .is_none());
    }

    fn num_nodes(&self) -> usize {
        2
    }

    fn connect_all(&self) -> bool {
        false
    }
}
