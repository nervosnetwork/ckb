use crate::{sleep, Net, Spec};
use log::info;

pub struct TransactionRelayBasic;

impl Spec for TransactionRelayBasic {
    fn run(&self, net: &Net) {
        info!("Running TransactionRelayBasic");
        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];
        let node2 = &net.nodes[2];

        info!("Generate new transaction on node1");
        node1.generate_block();
        let hash = node1.generate_transaction();

        info!("Waiting for relay");
        sleep(3);

        info!("Transaction should be relayed to node0 and node2");
        assert!(node0
            .rpc_client()
            .get_pool_transaction(hash.clone())
            .call()
            .unwrap()
            .is_some());

        assert!(node2
            .rpc_client()
            .get_pool_transaction(hash.clone())
            .call()
            .unwrap()
            .is_some());
    }
}
