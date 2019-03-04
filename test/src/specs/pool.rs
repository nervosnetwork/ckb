use crate::{sleep, Net, Spec};
use log::info;

pub struct PoolReconcile {}

impl Spec for PoolReconcile {
    fn run(&self, net: &Net) {
        info!("Running PoolReconcile");
        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];

        info!("Generate 1 block on node0");
        node0.generate_block();

        info!("Use generated block's cellbase as tx input");
        let hash = node0.generate_transaction();

        info!("Generate 2 blocks on node0");
        node0.generate_block();
        node0.generate_block();

        info!("Pool should be empty");
        assert!(node0
            .rpc_client()
            .get_pool_transaction(hash.clone())
            .call()
            .unwrap()
            .is_none());

        info!("Generate 4 blocks on node1");
        (0..4).for_each(|_| {
            node1.generate_block();
        });

        info!("Connect node0 to node1");
        node0.connect(node1);

        info!("Waiting for sync");
        sleep(10);

        info!("Tx should be re-added to node0's pool");
        assert!(node0
            .rpc_client()
            .get_pool_transaction(hash.clone())
            .call()
            .unwrap()
            .is_some());
    }

    fn num_nodes(&self) -> usize {
        2
    }

    fn connect_all(&self) -> bool {
        false
    }
}
