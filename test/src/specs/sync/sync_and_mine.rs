use crate::{Node, Spec};
use ckb_types::packed;
use std::thread::sleep;
use std::time::{Duration, Instant};

pub struct InboundMinedDuringSync;

impl Spec for InboundMinedDuringSync {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];

        node0.mine(1);
        node1.mine(1);

        // node0 is outbound
        // node1 is inbound
        node0.connect(node1);

        node0.mine(2000);
        let template = node1.rpc_client().get_block_template(None, None, None);
        let block = packed::Block::from(template).into_view();
        node1.submit_block(&block);
        ckb_logger::debug!("node1 submit block-{}", block.number());

        let last_check_time = Instant::now();
        let mut last_logging_time = Instant::now();
        let mut tip_number1 = node1.get_tip_block_number();
        loop {
            let tip_block0 = node0.get_tip_block();
            let tip_block1 = node1.get_tip_block();
            if tip_block0.number() == tip_block1.number() {
                break;
            }

            if last_logging_time.elapsed() > Duration::from_secs(10) {
                last_logging_time = Instant::now();
                ckb_logger::debug!(
                    "node0.tip_block.number = {}, node1.tip_block.number = {}",
                    tip_block0.number(),
                    tip_block1.number()
                );
            }

            if last_check_time.elapsed() > Duration::from_secs(30) {
                if tip_number1 == tip_block1.number() {
                    assert_eq!(
                        tip_block0.number(),
                        tip_block1.number(),
                        "node1 did not grow up in at past {} seconds",
                        last_check_time.elapsed().as_secs()
                    );
                }

                tip_number1 = tip_block1.number();
            }

            sleep(Duration::from_secs(1));
        }
    }
}
pub struct OutboundMinedDuringSync;

impl Spec for OutboundMinedDuringSync {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];

        node0.mine(1);
        node1.mine(1);

        // node0 is outbound
        // node1 is inbound
        node1.connect(node0);

        node0.mine(2000);
        let template = node1.rpc_client().get_block_template(None, None, None);
        let block = packed::Block::from(template).into_view();
        node1.submit_block(&block);
        ckb_logger::debug!("node1 submit block-{}", block.number());

        let last_check_time = Instant::now();
        let mut last_logging_time = Instant::now();
        let mut tip_number1 = node1.get_tip_block_number();
        loop {
            let tip_block0 = node0.get_tip_block();
            let tip_block1 = node1.get_tip_block();
            if tip_block0.number() == tip_block1.number() {
                break;
            }

            if last_logging_time.elapsed() > Duration::from_secs(10) {
                last_logging_time = Instant::now();
                ckb_logger::debug!(
                    "node0.tip_block.number = {}, node1.tip_block.number = {}",
                    tip_block0.number(),
                    tip_block1.number()
                );
            }

            if last_check_time.elapsed() > Duration::from_secs(30) {
                if tip_number1 == tip_block1.number() {
                    assert_eq!(
                        tip_block0.number(),
                        tip_block1.number(),
                        "node1 did not grow up in at past {} seconds",
                        last_check_time.elapsed().as_secs()
                    );
                }

                tip_number1 = tip_block1.number();
            }

            sleep(Duration::from_secs(1));
        }
    }
}

pub struct InboundSync;

impl Spec for InboundSync {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];

        node0.mine(1);
        node1.mine(1);

        // node0 is outbound
        // node1 is inbound
        node0.connect(node1);

        node0.mine(2000);

        let last_check_time = Instant::now();
        let mut last_logging_time = Instant::now();
        let mut tip_number1 = node1.get_tip_block_number();
        loop {
            let tip_block0 = node0.get_tip_block();
            let tip_block1 = node1.get_tip_block();
            if tip_block0.number() == tip_block1.number() {
                break;
            }

            if last_logging_time.elapsed() > Duration::from_secs(10) {
                last_logging_time = Instant::now();
                ckb_logger::debug!(
                    "node0.tip_block.number = {}, node1.tip_block.number = {}",
                    tip_block0.number(),
                    tip_block1.number()
                );
            }

            if last_check_time.elapsed() > Duration::from_secs(30) {
                if tip_number1 == tip_block1.number() {
                    assert_eq!(
                        tip_block0.number(),
                        tip_block1.number(),
                        "node1 did not grow up in at past {} seconds",
                        last_check_time.elapsed().as_secs()
                    );
                }

                tip_number1 = tip_block1.number();
            }

            sleep(Duration::from_secs(1));
        }
    }
}
pub struct OutboundSync;

impl Spec for OutboundSync {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];

        node0.mine(1);
        node1.mine(1);

        // node0 is outbound
        // node1 is inbound
        node1.connect(node0);

        node0.mine(2000);

        let last_check_time = Instant::now();
        let mut last_logging_time = Instant::now();
        let mut tip_number1 = node1.get_tip_block_number();
        loop {
            let tip_block0 = node0.get_tip_block();
            let tip_block1 = node1.get_tip_block();
            if tip_block0.number() == tip_block1.number() {
                break;
            }

            if last_logging_time.elapsed() > Duration::from_secs(10) {
                last_logging_time = Instant::now();
                ckb_logger::debug!(
                    "node0.tip_block.number = {}, node1.tip_block.number = {}",
                    tip_block0.number(),
                    tip_block1.number()
                );
            }

            if last_check_time.elapsed() > Duration::from_secs(30) {
                if tip_number1 == tip_block1.number() {
                    assert_eq!(
                        tip_block0.number(),
                        tip_block1.number(),
                        "node1 did not grow up in at past {} seconds",
                        last_check_time.elapsed().as_secs()
                    );
                }

                tip_number1 = tip_block1.number();
            }

            sleep(Duration::from_secs(1));
        }
    }
}
