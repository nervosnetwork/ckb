use crate::node::{make_bootnodes_for_all, waiting_for_sync};
use crate::util::mining::out_ibd_mode;
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

/// This test will start 5 nodes, and randomly restart 4 nodes in the middle of mining.
/// After all nodes are synced, the test is considered successful.
/// This test is used to test the robustness of the sync protocol.
/// If the sync protocol is not robust enough, the test will fail.
/// But this test is not a complete test, it can only test the robustness of the sync protocol to a certain extent.
/// Some weaknesses of this test:
/// 1. This test only consider the simple case of some nodes restarting in the middle of mining,
/// while other nodes are always mining correctly.
/// 2. This fault injection of restarting nodes is not comprehensive enough.
/// 3. Even if the test fails, we can't deterministically reproduce the same error.
/// We may need some foundationdb-like tools to deterministically reproduce the same error.
impl Spec for SyncChurn {
    crate::setup!(num_nodes: 5);

    fn run(&self, nodes: &mut Vec<Node>) {
        make_bootnodes_for_all(nodes);
        out_ibd_mode(nodes);

        let mut mining_nodes = nodes.clone();
        let mut churn_nodes = mining_nodes.split_off(2);

        let (restart_stopped_tx, restart_stopped_rx) = mpsc::channel();

        let mining_thread = thread::spawn(move || {
            let mut rng = rand::thread_rng();
            loop {
                let mining_node = select_random_node(&mut rng, &mut mining_nodes);
                mining_node.mine(1);
                // Because the test that waiting for nodes to sync has a implicit maximum waiting time
                // (currently 60 seconds, we can sync about 200 blocks per second, so a maxium blocks of 10000 is reasonable)
                // and the implicit waiting time is not long enough when there are too many blocks to sync,
                // so we stop mining when the tip block number is greater than 15000.
                // Otherwise nodes may not be able to sync within the implicit waiting time.
                let too_many_blocks = mining_node.get_tip_block_number() > 10000;
                if too_many_blocks || restart_stopped_rx.try_recv().is_ok() {
                    break;
                }
                waiting_for_sync(&mining_nodes);
            }
        });

        let restart_thread = thread::spawn(move || {
            let mut rng = rand::thread_rng();
            // It takes about 1 second to restart a node. So restarting nodes 100 times takes about 100 seconds.
            let num_restarts = 100;
            for _ in 0..num_restarts {
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
        waiting_for_sync(nodes);
    }
}
