use crate::utils::wait_until;
use crate::{Net, Spec};
use log::info;
use std::{thread::sleep, time::Duration};

pub struct IBDProcess;

impl Spec for IBDProcess {
    fn run(&self, net: Net) {
        info!("Running IBD process");

        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];
        let node2 = &net.nodes[2];
        let node3 = &net.nodes[3];
        let node4 = &net.nodes[4];
        let node5 = &net.nodes[5];
        let node6 = &net.nodes[6];

        node0.connect(node1);
        node0.connect(node2);
        node0.connect(node3);
        node0.connect(node4);
        // will never connect
        node0.connect_uncheck(node5);
        node0.connect_uncheck(node6);

        sleep(Duration::from_secs(5));

        let rpc_client = node0.rpc_client();
        let ret = wait_until(10, || {
            let peers = rpc_client.get_peers();
            peers.len() == 4
        });

        if !ret {
            panic!("refuse to connect fail");
        }
    }

    fn num_nodes(&self) -> usize {
        7
    }

    fn connect_all(&self) -> bool {
        false
    }
}
