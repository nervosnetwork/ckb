use crate::{Node, Spec};
use ckb_types::{packed, prelude::*};
use std::thread::sleep;
use std::time::{Duration, Instant};

fn run_sync_and_mine(nodes: &[Node], node0_connects_node1: bool, mine_on_node1: bool) {
    let node0 = &nodes[0];
    let node1 = &nodes[1];

    node0.mine(1);
    node1.mine_with(1, |builder| builder.nonce(1).build());
    assert_ne!(
        node0.get_tip_block().hash(),
        node1.get_tip_block().hash(),
        "node0 and node1 must diverge at block #1"
    );

    if node0_connects_node1 {
        node0.connect(node1);
    } else {
        node1.connect(node0);
    }

    node0.mine(2000);
    if mine_on_node1 {
        let template = node1.rpc_client().get_block_template(None, None, None);
        let block = packed::Block::from(template).into_view();
        node1.submit_block(&block);
        ckb_logger::debug!("node1 submit block-{}", block.number());
    }

    let mut last_check_time = Instant::now();
    let mut last_logging_time = Instant::now();
    let mut tip_number1 = node1.get_tip_block_number();
    loop {
        let tip_header0 = node0.rpc_client().get_tip_header();
        let tip_header1 = node1.rpc_client().get_tip_header();
        if tip_header0 == tip_header1 {
            break;
        }

        if last_logging_time.elapsed() > Duration::from_secs(10) {
            last_logging_time = Instant::now();
            ckb_logger::debug!(
                "node0.tip_block.number = {}, node1.tip_block.number = {}",
                tip_header0.inner.number.value(),
                tip_header1.inner.number.value()
            );
        }

        if last_check_time.elapsed() > Duration::from_secs(30) {
            if tip_number1 == tip_header1.inner.number.value() {
                assert_eq!(
                    tip_header0,
                    tip_header1,
                    "node1 did not grow up in past {} seconds",
                    last_check_time.elapsed().as_secs()
                );
            }

            tip_number1 = tip_header1.inner.number.value();
            last_check_time = Instant::now();
        }

        sleep(Duration::from_millis(50));
    }
}

pub struct InboundMinedDuringSync;

impl Spec for InboundMinedDuringSync {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        run_sync_and_mine(nodes, true, true);
    }
}

pub struct OutboundMinedDuringSync;

impl Spec for OutboundMinedDuringSync {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        run_sync_and_mine(nodes, false, true);
    }
}

pub struct InboundSync;

impl Spec for InboundSync {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        run_sync_and_mine(nodes, true, false);
    }
}

pub struct OutboundSync;

impl Spec for OutboundSync {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        run_sync_and_mine(nodes, false, false);
    }
}
