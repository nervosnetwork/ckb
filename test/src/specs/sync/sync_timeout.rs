use crate::node::{disconnect_all, waiting_for_sync};
use crate::{Node, Spec};
use log::info;

pub struct SyncTimeout;

impl Spec for SyncTimeout {
    crate::setup!(num_nodes: 5);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];
        let node3 = &nodes[3];
        let node4 = &nodes[4];

        info!("Generate 2 blocks on node0");
        node0.generate_blocks(2);

        info!("Connect all nodes");
        node1.connect(node0);
        node2.connect(node0);
        node3.connect(node0);
        node4.connect(node0);
        waiting_for_sync(nodes);

        info!("Disconnect all nodes");
        disconnect_all(nodes);

        info!("Generate 200 blocks on node0");
        node0.generate_blocks(200);

        node0.connect(node1);
        info!("Waiting for node0 and node1 sync");
        node0.waiting_for_sync(node1, 202);

        info!("Generate 200 blocks on node1");
        node1.generate_blocks(200);

        node2.connect(node0);
        node2.connect(node1);
        node3.connect(node0);
        node3.connect(node1);
        node4.connect(node0);
        node4.connect(node1);
        info!("Waiting for all nodes sync");
        waiting_for_sync(nodes);
    }
}
