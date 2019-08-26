use crate::utils::{assert_tx_pool_size, waiting_for_sync};
use crate::{Net, Spec};
use log::info;

pub struct PoolResurrect;

impl Spec for PoolResurrect {
    crate::name!("pool_resurrect");

    crate::setup!(num_nodes: 2, connect_all: false);

    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];

        info!("Generate 1 block on node0");
        node0.generate_block();

        info!("Generate 6 txs on node0");
        let mut txs_hash = Vec::new();
        let mut hash = node0.generate_transaction();
        txs_hash.push(hash.clone());

        (0..5).for_each(|_| {
            let tx = node0.new_transaction(hash.clone());
            hash = node0
                .rpc_client()
                .send_transaction(tx.clone().data().into());
            txs_hash.push(hash.clone());
        });

        info!("Generate 3 more blocks on node0");
        node0.generate_blocks(3);

        info!("Pool should be empty");
        let tx_pool_info = node0.rpc_client().tx_pool_info();
        assert!(tx_pool_info.pending.0 == 0);

        info!("Generate 5 blocks on node1");
        node1.generate_blocks(5);

        info!("Connect node0 to node1, waiting for sync");
        node0.connect(node1);
        waiting_for_sync(&net.nodes, 5);

        info!("6 txs should be returned to node0 pending pool");
        assert_tx_pool_size(node0, txs_hash.len() as u64, 0);

        info!("Generate 2 blocks on node0, 6 txs should be added to proposed pool");
        node0.generate_blocks(2);
        assert_tx_pool_size(node0, 0, txs_hash.len() as u64);

        info!("Generate 1 block on node0, 6 txs should be included in this block");
        node0.generate_block();
        assert_tx_pool_size(node0, 0, 0);
    }
}
