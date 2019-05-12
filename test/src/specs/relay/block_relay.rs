use crate::utils::wait_until;
use crate::{Net, Spec};
use log::info;

pub struct BlockRelayBasic;

impl Spec for BlockRelayBasic {
    fn run(&self, net: Net) {
        info!("Running BlockRelayBasic");
        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];
        let node2 = &net.nodes[2];

        info!("Generate new block on node1");
        let hash = node1.generate_block();

        let mut rpc_client = node0.rpc_client();
        let ret = wait_until(10, || {
            rpc_client.get_block(hash.clone()).call().unwrap().is_some()
        });
        assert!(ret, "Block should be relayed to node0");

        let mut rpc_client = node2.rpc_client();
        let ret = wait_until(10, || {
            rpc_client.get_block(hash.clone()).call().unwrap().is_some()
        });
        assert!(ret, "Block should be relayed to node2");
    }
}
