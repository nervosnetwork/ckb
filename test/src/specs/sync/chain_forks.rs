use crate::{Net, Spec};
use ckb_app_config::CKBAppConfig;
use log::info;

pub struct ChainFork1;

impl Spec for ChainFork1 {
    //                  1    2    3    4
    // node0 genesis -> A -> B -> C
    // node1                 \ -> D -> E
    fn run(&self, net: Net) {
        info!("Running ChainFork1");
        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];

        info!("Generate 2 blocks (A, B) on node0");
        node0.generate_blocks(2);

        info!("Connect node0 to node1");
        node0.connect(node1);
        assert_eq!(2, node0.waiting_for_sync(node1, 10));
        info!("Disconnect node1");
        node0.disconnect(node1);

        info!("Generate 1 block (C) on node0");
        node0.generate_blocks(1);
        info!("Generate 2 blocks (D, E) on node1");
        node1.generate_blocks(2);

        info!("Reconnect node0 to node1");
        node0.connect(node1);
        assert_eq!(4, net.waiting_for_sync(10));
    }

    fn num_nodes(&self) -> usize {
        2
    }

    fn connect_all(&self) -> bool {
        false
    }
}

pub struct ChainFork2;

impl Spec for ChainFork2 {
    //                  1    2    3     4    5
    // node0 genesis -> A -> B -> C
    // node1                 \ -> D ->  E
    // node2                 \ -> C  -> F -> G
    fn run(&self, net: Net) {
        info!("Running ChainFork2");
        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];
        let node2 = &net.nodes[2];

        info!("Generate 2 blocks (A, B) on node0");
        node0.generate_blocks(2);

        info!("Connect all nodes");
        net.connect_all();
        assert_eq!(2, net.waiting_for_sync(10));

        info!("Disconnect node1");
        node0.disconnect(node1);
        node2.disconnect(node1);
        node0.connect(node2);
        info!("Generate 1 block (C) on node0");
        node0.generate_blocks(1);
        assert_eq!(3, node0.waiting_for_sync(node2, 10));
        info!("Disconnect node2");
        node0.disconnect(node2);

        info!("Generate 2 blocks (D, E) on node1");
        node1.generate_blocks(2);
        info!("Reconnect node1");
        node0.connect(node1);
        assert_eq!(4, node0.waiting_for_sync(node1, 10));

        info!("Generate 2 blocks (F, G) on node2");
        node2.generate_blocks(2);
        info!("Reconnect node2");
        node0.connect(node2);
        node1.connect(node2);
        assert_eq!(5, net.waiting_for_sync(10));
    }

    fn num_nodes(&self) -> usize {
        3
    }

    fn connect_all(&self) -> bool {
        false
    }

    // workaround to disable node discovery
    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig) -> ()> {
        Box::new(|config| config.network.connect_outbound_interval_secs = 100000)
    }
}
