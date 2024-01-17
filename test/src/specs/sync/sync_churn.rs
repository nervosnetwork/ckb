use crate::node::{make_bootnodes_for_all, waiting_for_sync};
use crate::{Node, Spec};
use ckb_logger::info;
use rand::Rng;

fn select_random_node<'a, R: Rng>(rng: &mut R, nodes: &'a mut [Node]) -> &'a mut Node {
    let index = rng.gen_range(0, nodes.len());
    &mut nodes[index]
}

fn randomly_restart<R: Rng>(rng: &mut R, restart_probilibity: f64, node: &mut Node) {
    let should_restart = rng.gen_bool(restart_probilibity);
    if should_restart {
        node.stop();
        node.start();
    }
}

pub struct SyncChurn;

impl Spec for SyncChurn {
    crate::setup!(num_nodes: 5);

    fn run(&self, nodes: &mut Vec<Node>) {
        make_bootnodes_for_all(nodes);

        let mut rng = rand::thread_rng();
        let (mining_nodes, churn_nodes) = nodes.split_at_mut(1);
        for _ in 0..1000 {
            const RESTART_PROBABILITY: f64 = 0.1;
            let mining_node = select_random_node(&mut rng, mining_nodes);
            mining_node.mine(1);
            let node = select_random_node(&mut rng, churn_nodes);
            randomly_restart(&mut rng, RESTART_PROBABILITY, node);
        }

        info!("Waiting for all nodes sync");
        waiting_for_sync(&nodes);
    }
}
