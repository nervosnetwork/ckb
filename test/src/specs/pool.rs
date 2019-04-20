use crate::{sleep, Net, Spec};
use jsonrpc_types::{Action, TxTrace};
use log::info;

pub struct PoolReconcile;

impl Spec for PoolReconcile {
    fn run(&self, net: Net) {
        info!("Running PoolReconcile");
        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];

        info!("Generate 1 block on node0");
        node0.generate_block();

        info!("Use generated block's cellbase as tx input");
        let hash = node0.generate_transaction();

        info!("Generate 3 more blocks on node0");
        node0.generate_block();
        node0.generate_block();
        node0.generate_block();

        info!("Pool should be empty");
        assert!(node0
            .rpc_client()
            .get_pool_transaction(hash.clone())
            .call()
            .unwrap()
            .is_none());

        info!("Generate 5 blocks on node1");
        (0..5).for_each(|_| {
            node1.generate_block();
        });

        info!("Connect node0 to node1");
        node0.connect(node1);

        info!("Waiting for sync");
        sleep(10);

        info!("Tx should be mined");
        assert!(node0
            .rpc_client()
            .get_pool_transaction(hash.clone())
            .call()
            .unwrap()
            .is_none());
    }

    fn num_nodes(&self) -> usize {
        2
    }

    fn connect_all(&self) -> bool {
        false
    }
}

pub struct PoolTrace;

impl Spec for PoolTrace {
    fn run(&self, net: Net) {
        info!("Running PoolTrace");
        let node0 = &net.nodes[0];

        info!("Generate 1 block on node0");
        node0.generate_block();

        info!("Use generated block's cellbase as tx input");
        let hash = node0.send_traced_transaction();

        info!("Generate 3 more blocks on node0");
        node0.generate_block();
        node0.generate_block();
        node0.generate_block();

        let actions: Vec<_> = node0
            .rpc_client()
            .get_transaction_trace(hash)
            .call()
            .unwrap()
            .unwrap()
            .iter()
            .map(TxTrace::action)
            .cloned()
            .collect();

        info!("TxTrace actions should contains AddPending and Committed");
        assert!(actions.contains(&Action::AddPending) && actions.contains(&Action::Committed));
    }

    fn num_nodes(&self) -> usize {
        1
    }

    fn connect_all(&self) -> bool {
        false
    }
}
