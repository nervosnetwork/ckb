use crate::utils::wait_until;
use crate::{Net, Spec};
use log::info;

pub struct BlockRelayBasic;

impl Spec for BlockRelayBasic {
    crate::name!("block_relay_basic");

    crate::setup!(num_nodes: 3);

    fn run(&self, net: Net) {
        net.exit_ibd_mode();
        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];
        let node2 = &net.nodes[2];

        info!("Generate new block on node1");
        let hash = node1.generate_block();

        let rpc_client = node0.rpc_client();
        let ret = wait_until(10, || rpc_client.get_block(hash.clone()).is_some());
        assert!(ret, "Block should be relayed to node0");

        let rpc_client = node2.rpc_client();
        let ret = wait_until(10, || rpc_client.get_block(hash.clone()).is_some());
        assert!(ret, "Block should be relayed to node2");
    }
}
