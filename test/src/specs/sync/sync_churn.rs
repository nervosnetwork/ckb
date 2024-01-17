use crate::node::{make_bootnodes_for_all, waiting_for_sync};
use crate::{Node, Spec};
use ckb_logger::info;
use rand::Rng;
use std::sync::mpsc;
use std::thread;

fn select_random_node<'a, R: Rng>(rng: &mut R, nodes: &'a mut [Node]) -> &'a mut Node {
    let index = rng.gen_range(0, nodes.len());
    &mut nodes[index]
}

pub struct SyncChurn;

impl Spec for SyncChurn {
    crate::setup!(num_nodes: 5);

    fn run(&self, nodes: &mut Vec<Node>) {
        make_bootnodes_for_all(nodes);

        let mut mining_nodes = nodes.clone();
        let mut churn_nodes = mining_nodes.split_off(1);

        let (restart_stopped_tx, restart_stopped_rx) = mpsc::channel();

        let mining_thread = thread::spawn(move || {
            let mut rng = rand::thread_rng();
            loop {
                let mining_node = select_random_node(&mut rng, &mut mining_nodes);
                mining_node.mine(1);
                waiting_for_sync(&mining_nodes);
                if restart_stopped_rx.try_recv().is_ok() {
                    break;
                }
            }
        });

        let restart_thread = thread::spawn(move || {
            let mut rng = rand::thread_rng();
            for _ in 0..100 {
                let node = select_random_node(&mut rng, &mut churn_nodes);
                info!("Restarting node {}", node.node_id());
                node.stop();
                node.start();
            }
            restart_stopped_tx.send(()).unwrap();
        });

        mining_thread.join().unwrap();
        restart_thread.join().unwrap();

        info!("Waiting for all nodes sync");
        waiting_for_sync(&nodes);
    }
}
