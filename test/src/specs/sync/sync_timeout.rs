use crate::{Net, Spec};
use ckb_app_config::CKBAppConfig;
use log::info;

pub struct SyncTimeout;

impl Spec for SyncTimeout {
    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];
        let node2 = &net.nodes[2];
        let node3 = &net.nodes[3];
        let node4 = &net.nodes[4];

        info!("Generate 2 blocks on node0");
        node0.generate_blocks(2);

        info!("Connect all nodes");
        node0.connect(node1);
        node0.connect(node2);
        node0.connect(node3);
        node0.connect(node4);
        net.waiting_for_sync(2);

        info!("Disconnect all nodes");
        net.disconnect_all();

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
        net.waiting_for_sync(402);
    }

    fn num_nodes(&self) -> usize {
        5
    }

    fn connect_all(&self) -> bool {
        false
    }

    // workaround to disable node discovery
    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig) -> ()> {
        Box::new(|config| config.network.connect_outbound_interval_secs = 100_000)
    }
}
