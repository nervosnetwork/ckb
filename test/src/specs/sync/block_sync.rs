use crate::{Net, Spec};
use log::info;

pub struct BlockSyncBasic;

impl Spec for BlockSyncBasic {
    fn run(&self, net: Net) {
        info!("Running BlockSyncBasic");
        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];

        info!("Generate 3 blocks on node0");
        (0..3).for_each(|_| {
            node0.generate_block();
        });

        info!("Connect node0 to node1");
        node0.connect(node1);

        net.waiting_for_sync(10);

        info!("Node1 should be synced to same block number with node0");
        let number0 = node0.rpc_client().get_tip_block_number().call().unwrap().0;
        let number1 = node0.rpc_client().get_tip_block_number().call().unwrap().0;
        assert_eq!(number0, number1);
    }

    fn num_nodes(&self) -> usize {
        2
    }

    fn connect_all(&self) -> bool {
        false
    }
}
