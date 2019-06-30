use crate::utils::wait_until;
use crate::{Net, Spec};
use log::info;

pub struct BlockRelayBasic;

impl Spec for BlockRelayBasic {
    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];
        let node2 = &net.nodes[2];
        // generate 1 block to exit IBD mode.
        let block = node0.new_block(None, None, None);
        node0.submit_block(&block);
        node1.submit_block(&block);
        node2.submit_block(&block);

        info!("Generate new block on node1");
        let hash = node1.generate_block();

        let rpc_client = node0.rpc_client();
        let ret = wait_until(10, || rpc_client.get_block(hash.clone()).is_some());
        assert!(ret, "Block should be relayed to node0");

        let rpc_client = node2.rpc_client();
        let ret = wait_until(10, || rpc_client.get_block(hash.clone()).is_some());
        assert!(ret, "Block should be relayed to node2");
    }

    fn num_nodes(&self) -> usize {
        3
    }
}
