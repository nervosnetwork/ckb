use crate::{sleep, Net, Spec};
use log::info;

pub struct BlockRelayBasic {}

impl Spec for BlockRelayBasic {
    fn run(&self, net: &Net) {
        info!("Running BlockRelayBasic");
        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];
        let node2 = &net.nodes[2];

        info!("Generate new block on node1");
        let hash = node1.generate_block();

        info!("Waiting for relay");
        sleep(3);

        info!("Block should be relayed to node0 and node2");
        assert!(node0
            .rpc_client()
            .get_block(hash.clone())
            .call()
            .unwrap()
            .is_some());

        assert!(node2
            .rpc_client()
            .get_block(hash.clone())
            .call()
            .unwrap()
            .is_some());
    }
}
