use crate::{Net, Spec};
use ckb_app_config::CKBAppConfig;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::transaction::TransactionBuilder;
use ckb_core::{capacity_bytes, Capacity};
use log::info;

pub struct ChainFork1;

impl Spec for ChainFork1 {
    // Test normal fork
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
        node0.waiting_for_sync(node1, 2, 10);
        info!("Disconnect node1");
        node0.disconnect(node1);

        info!("Generate 1 block (C) on node0");
        node0.generate_blocks(1);
        info!("Generate 2 blocks (D, E) on node1");
        node1.generate_blocks(2);

        info!("Reconnect node0 to node1");
        node0.connect(node1);
        net.waiting_for_sync(4, 10);
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
    // Test normal fork switch back
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
        net.waiting_for_sync(2, 10);
        info!("Disconnect all nodes");
        net.disconnect_all();

        info!("Generate 1 block (C) on node0");
        node0.generate_blocks(1);
        node0.connect(node2);
        node0.waiting_for_sync(node2, 3, 10);
        info!("Disconnect node2");
        node0.disconnect(node2);

        info!("Generate 2 blocks (D, E) on node1");
        node1.generate_blocks(2);
        info!("Reconnect node1");
        node0.connect(node1);
        node0.waiting_for_sync(node1, 4, 10);

        info!("Generate 2 blocks (F, G) on node2");
        node2.generate_blocks(2);
        info!("Reconnect node2");
        node0.connect(node2);
        node1.connect(node2);
        net.waiting_for_sync(5, 10);
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

pub struct ChainFork3;

impl Spec for ChainFork3 {
    // Test invalid cellbase fork (in block F)
    //                  1    2    3     4    5
    // node0 genesis -> A -> B -> C
    // node1                 \ -> D  -> E -> F
    // node2                 \ -> C  -> G
    fn run(&self, net: Net) {
        info!("Running ChainFork3");
        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];
        let node2 = &net.nodes[2];

        info!("Generate 2 blocks (A, B) on node0");
        node0.generate_blocks(2);

        info!("Connect all nodes");
        net.connect_all();
        net.waiting_for_sync(2, 10);

        info!("Disconnect all nodes");
        net.disconnect_all();

        info!("Generate 1 block (C) on node0");
        node0.generate_blocks(1);
        node0.connect(node2);
        node0.waiting_for_sync(node2, 3, 10);
        info!("Disconnect node2");
        node0.disconnect(node2);

        info!("Generate 2 blocks (D, E) on node1");
        node1.generate_blocks(2);
        info!("Generate 1 block (F) with invalid reward cellbase on node1");
        let block = node1.new_block(None, None, None);
        let invalid_block = modify_block(block);
        node1.process_block_without_verify(&invalid_block);

        info!("Reconnect node1");
        node0.connect(node1);

        info!("Generate 1 block (G) on node2");
        node2.generate_blocks(1);
        info!("Reconnect node2");
        node0.connect(node2);
        node1.connect(node2);
        node0.waiting_for_sync(node2, 4, 10);
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

fn modify_block(block: Block) -> Block {
    let (header, uncles, mut transactions, proposals) = (
        block.header().clone(),
        block.uncles().to_vec(),
        block.transactions().to_vec(),
        block.proposals().to_vec(),
    );

    let transaction = transactions[0].clone();
    let mut output = transaction.outputs()[0].clone();
    output.capacity = output.capacity.safe_add(capacity_bytes!(1)).unwrap();

    let transaction = TransactionBuilder::from_transaction(transaction)
        .outputs_clear()
        .output(output)
        .build();
    transactions[0] = transaction;

    BlockBuilder::default()
        .header(header)
        .uncles(uncles)
        .transactions(transactions)
        .proposals(proposals)
        .build()
}
