use crate::{Node, Spec};

use ckb_logger::info;
use rand::{thread_rng, Rng};

pub struct RandomlyKill;

impl Spec for RandomlyKill {
    crate::setup!(num_nodes: 1);

    fn run(&self, nodes: &mut Vec<Node>) {
        let mut rng = thread_rng();
        let node = &mut nodes[0];
        for _ in 0..rng.gen_range(10, 20) {
            let n = rng.gen_range(0, 10);
            // TODO: the kill of child process and mining are actually sequential here
            // We need to find some way to so these two things in parallel.
            // It would be great if we can kill and start the node externally (instead of writing
            // rust code to manage all the nodes, because in that case we will have to fight
            // ownership rules, and monitor node).
            if n != 0 {
                info!("Mining {} blocks", n);
                node.mine(n);
            }
            info!("Stop the node");
            node.stop();
            info!("Start the node");
            node.start();
        }
    }
}
