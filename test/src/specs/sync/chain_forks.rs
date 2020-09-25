use crate::node::{disconnect_all, exit_ibd_mode, waiting_for_sync};
use crate::util::check::{is_transaction_committed, is_transaction_pending};
use crate::{Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_types::{
    core::{capacity_bytes, BlockView, Capacity, TransactionView},
    h256,
    prelude::*,
    H256,
};
use failure::_core::time::Duration;
use log::info;
use std::thread::sleep;

pub struct ChainFork1;

impl Spec for ChainFork1 {
    crate::setup!(num_nodes: 2);

    // Test normal fork
    //                  1    2    3    4
    // node0 genesis -> A -> B -> C
    // node1                 \ -> D -> E
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];

        info!("Generate 2 blocks (A, B) on node0");
        node0.generate_blocks(2);

        info!("Connect node0 to node1");
        node1.connect(node0);
        node0.waiting_for_sync(node1, 2);
        info!("Disconnect node1");
        node0.disconnect(node1);

        info!("Generate 1 block (C) on node0");
        node0.generate_blocks(1);
        info!("Generate 2 blocks (D, E) on node1");
        node1.generate_blocks(2);

        info!("Reconnect node0 to node1");
        node0.connect(node1);
        waiting_for_sync(nodes);
    }

    // workaround to disable node discovery
    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.network.connect_outbound_interval_secs = 100_000;
    }
}

pub struct ChainFork2;

impl Spec for ChainFork2 {
    crate::setup!(num_nodes: 3);

    // Test normal fork switch back
    //                  1    2    3    4    5
    // node0 genesis -> A -> B -> C
    // node1                 \ -> D -> E
    // node2                 \ -> C -> F -> G
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];

        info!("Generate 2 blocks (A, B) on node0");
        node0.generate_blocks(2);

        info!("Connect all nodes");
        node1.connect(node0);
        node2.connect(node0);
        waiting_for_sync(nodes);
        info!("Disconnect all nodes");
        disconnect_all(nodes);

        info!("Generate 1 block (C) on node0");
        node0.generate_blocks(1);
        node0.connect(node2);
        node0.waiting_for_sync(node2, 3);
        info!("Disconnect node2");
        node0.disconnect(node2);

        info!("Generate 2 blocks (D, E) on node1");
        node1.generate_blocks(2);
        info!("Reconnect node1");
        node0.connect(node1);
        node0.waiting_for_sync(node1, 4);

        info!("Generate 2 blocks (F, G) on node2");
        node2.generate_blocks(2);
        info!("Reconnect node2");
        node0.connect(node2);
        node1.connect(node2);
        waiting_for_sync(nodes);
    }
}

pub struct ChainFork3;

impl Spec for ChainFork3 {
    crate::setup!(num_nodes: 3);

    // Test invalid cellbase reward fork (in block F)
    //                  1    2    3    4    5
    // node0 genesis -> A -> B -> C
    // node1                 \ -> D -> E -> F
    // node2                 \ -> C -> G
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];

        info!("Generate DEFAULT_TX_PROPOSAL_WINDOW.1 + 2 blocks (A, B) on node0");
        node0.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);

        info!("Connect all nodes");
        node1.connect(node0);
        node2.connect(node0);
        waiting_for_sync(nodes);

        info!("Disconnect all nodes");
        disconnect_all(nodes);

        info!("Generate 1 block (C) on node0");
        node0.generate_blocks(1);
        node0.connect(node2);
        node0.waiting_for_sync(node2, DEFAULT_TX_PROPOSAL_WINDOW.1 + 3);
        info!("Disconnect node2");
        node0.disconnect(node2);

        info!("Generate 2 blocks (D, E) on node1");
        node1.generate_blocks(2);
        info!("Generate 1 block (F) with invalid reward cellbase on node1");
        let block = node1.new_block(None, None, None);
        let invalid_block = modify_block_transaction(block, 0, |transaction| {
            let old_output = transaction
                .outputs()
                .as_reader()
                .get(0)
                .unwrap()
                .to_entity();
            let old_capacity: Capacity = old_output.capacity().unpack();
            let new_output = old_output
                .as_builder()
                .capacity(old_capacity.safe_add(capacity_bytes!(1)).unwrap().pack())
                .build();
            transaction
                .as_advanced_builder()
                .set_outputs(vec![new_output])
                .build()
        });
        node1.process_block_without_verify(&invalid_block, false);
        assert_eq!(15, node1.rpc_client().get_tip_block_number());

        info!("Reconnect node1 and node1 should be banned");
        node0.connect_and_wait_ban(node1);

        info!("Generate 1 block (G) on node2");
        node2.generate_blocks(1);
        info!("Reconnect node2");
        node2.connect(node0);
        node2.connect_and_wait_ban(node1);
        node0.waiting_for_sync(node2, DEFAULT_TX_PROPOSAL_WINDOW.1 + 4);
    }
}

pub struct ChainFork4;

impl Spec for ChainFork4 {
    crate::setup!(num_nodes: 3);

    // Test invalid cellbase capacity overflow fork (in block F)
    //                  1    2    3    4    5
    // node0 genesis -> A -> B -> C
    // node1                 \ -> D -> E -> F
    // node2                 \ -> C -> G
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];

        info!("Generate 2 blocks (A, B) on node0");
        node0.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);

        info!("Connect all nodes");
        node1.connect(node0);
        node2.connect(node0);
        waiting_for_sync(nodes);

        info!("Disconnect all nodes");
        disconnect_all(nodes);

        info!("Generate 1 block (C) on node0");
        node0.generate_blocks(1);
        node0.connect(node2);
        node0.waiting_for_sync(node2, DEFAULT_TX_PROPOSAL_WINDOW.1 + 3);
        info!("Disconnect node2");
        node0.disconnect(node2);

        info!("Generate 2 blocks (D, E) on node1");
        node1.generate_blocks(2);
        info!("Generate 1 block (F) with capacity overflow cellbase on node1");
        let block = node1.new_block(None, None, None);
        let invalid_block = modify_block_transaction(block, 0, |transaction| {
            let output = transaction
                .outputs()
                .as_reader()
                .get(0)
                .unwrap()
                .to_entity()
                .as_builder()
                .capacity(capacity_bytes!(1).pack())
                .build();
            transaction
                .as_advanced_builder()
                .set_outputs(vec![output])
                .build()
        });
        node1.process_block_without_verify(&invalid_block, false);
        assert_eq!(15, node1.rpc_client().get_tip_block_number());

        info!("Reconnect node1 and node1 should be banned");
        node0.connect_and_wait_ban(node1);

        info!("Generate 1 block (G) on node2");
        node2.generate_blocks(1);
        info!("Reconnect node2");
        node2.connect(node0);
        node2.connect_and_wait_ban(node1);
        node0.waiting_for_sync(node2, DEFAULT_TX_PROPOSAL_WINDOW.1 + 4);
    }
}

pub struct ChainFork5;

impl Spec for ChainFork5 {
    crate::setup!(num_nodes: 3);

    // Test dead cell fork (spent A cellbase in E, and spent A cellbase in F again)
    //                  1    2    3    4    5
    // node0 genesis -> A -> B -> C
    // node1                 \ -> D -> E -> F
    // node2                 \ -> C -> G
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];

        info!("Generate DEFAULT_TX_PROPOSAL_WINDOW +2 block (A) on node0");
        node0.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);
        info!("Generate 1 block (B) on node0, proposal spent A cellbase transaction");
        let transaction = node0.new_transaction_spend_tip_cellbase();
        node0.submit_transaction(&transaction);
        node0.generate_blocks(1);
        info!("Connect all nodes");
        node1.connect(node0);
        node2.connect(node0);
        waiting_for_sync(nodes);

        info!("Disconnect all nodes");
        disconnect_all(nodes);

        info!("Generate 1 block (C) on node0");
        node0.generate_blocks(1);
        node0.connect(node2);
        node0.waiting_for_sync(node2, DEFAULT_TX_PROPOSAL_WINDOW.1 + 4);
        info!("Disconnect node2");
        node0.disconnect(node2);

        info!("Generate 1 blocks (D) on node1");
        node1.generate_blocks(1);
        info!("Generate 1 blocks (E) with transaction on node1");
        let block = {
            let block = node1.new_block(None, None, None);
            // transaction may be broadcasted to node1 already
            if block.transactions().contains(&transaction) {
                block
            } else {
                block
                    .as_advanced_builder()
                    .transaction(transaction.clone())
                    .build()
            }
        };
        node1.submit_block(&block);
        assert_eq!(15, node1.rpc_client().get_tip_block_number());
        info!("Generate 1 blocks (F) with spent transaction on node1");
        let block = node1.new_block(None, None, None);
        let invalid_block = block.as_advanced_builder().transaction(transaction).build();
        node1.process_block_without_verify(&invalid_block, false);
        assert_eq!(16, node1.rpc_client().get_tip_block_number());

        info!("Reconnect node1 and node1 should be banned");
        node0.connect_and_wait_ban(node1);

        info!("Generate 1 block (G) on node2");
        node2.generate_blocks(1);
        info!("Reconnect node2");
        node2.connect(node0);
        node2.connect_and_wait_ban(node1);
        node0.waiting_for_sync(node2, DEFAULT_TX_PROPOSAL_WINDOW.1 + 5);
    }
}

pub struct ChainFork6;

impl Spec for ChainFork6 {
    crate::setup!(num_nodes: 3);

    // Test fork spending the outpoint of a non-existent transaction (in block F)
    //                  1    2    3    4    5
    // node0 genesis -> A -> B -> C
    // node1                 \ -> D -> E -> F
    // node2                 \ -> C -> G
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];

        info!("Generate 2 blocks (A, B) on node0");
        node0.generate_blocks(2);

        info!("Connect all nodes");
        node1.connect(node0);
        node2.connect(node0);
        waiting_for_sync(nodes);

        info!("Disconnect all nodes");
        disconnect_all(nodes);

        info!("Generate 1 block (C) on node0");
        node0.generate_blocks(1);
        node0.connect(node2);
        node0.waiting_for_sync(node2, 3);
        info!("Disconnect node2");
        node0.disconnect(node2);

        info!("Generate 2 blocks (D, E) on node1");
        node1.generate_blocks(2);
        info!("Generate 1 block (F) with spending non-existent transaction on node1");
        let block = node1.new_block(None, None, None);
        let invalid_transaction = node1.new_transaction(h256!("0x1").pack());
        let invalid_block = block
            .as_advanced_builder()
            .transaction(invalid_transaction)
            .build();
        node1.process_block_without_verify(&invalid_block, false);
        assert_eq!(5, node1.rpc_client().get_tip_block_number());

        info!("Reconnect node1 and node1 should be banned");
        node0.connect_and_wait_ban(node1);

        info!("Generate 1 block (G) on node2");
        node2.generate_blocks(1);
        info!("Reconnect node2");
        node2.connect(node0);
        node2.connect_and_wait_ban(node1);
        node0.waiting_for_sync(node2, 4);
    }
}

pub struct ChainFork7;

impl Spec for ChainFork7 {
    crate::setup!(num_nodes: 3);

    // Test fork spending the outpoint of an invalid index (in block F)
    //                  1    2    3    4    5
    // node0 genesis -> A -> B -> C
    // node1                 \ -> D -> E -> F
    // node2                 \ -> C -> G
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];

        info!("Generate 12 blocks (A, B) on node0");
        node0.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);

        info!("Connect all nodes");
        node1.connect(node0);
        node2.connect(node0);
        waiting_for_sync(nodes);

        info!("Disconnect all nodes");
        disconnect_all(nodes);

        info!("Generate 1 block (C) on node0");
        node0.generate_blocks(1);
        node0.connect(node2);
        node0.waiting_for_sync(node2, DEFAULT_TX_PROPOSAL_WINDOW.1 + 3);
        info!("Disconnect node2");
        node0.disconnect(node2);

        info!("Generate 2 blocks (D, E) on node1");
        node1.generate_blocks(2);
        info!("Generate 1 block (F) with spending invalid index transaction on node1");
        let block = node1.new_block(None, None, None);
        let transaction = node1.new_transaction_spend_tip_cellbase();
        let input = transaction.inputs().as_reader().get(0).unwrap().to_entity();
        let previous_output = input
            .previous_output()
            .as_builder()
            .index(999u32.pack())
            .build();
        let input = input.as_builder().previous_output(previous_output).build();
        let invalid_transaction = transaction
            .as_advanced_builder()
            .set_inputs(vec![input])
            .build();
        let invalid_block = block
            .as_advanced_builder()
            .transaction(invalid_transaction)
            .build();
        node1.process_block_without_verify(&invalid_block, false);
        assert_eq!(15, node1.rpc_client().get_tip_block_number());

        info!("Reconnect node1 and node1 should be banned");
        node0.connect_and_wait_ban(node1);

        info!("Generate 1 block (G) on node2");
        node2.generate_blocks(1);
        info!("Reconnect node2");
        node2.connect(node0);
        node2.connect_and_wait_ban(node1);
        node0.waiting_for_sync(node2, DEFAULT_TX_PROPOSAL_WINDOW.1 + 4);
    }
}

pub struct LongForks;

impl Spec for LongForks {
    crate::setup!(num_nodes: 3);

    // Case: Two nodes has different long forks should be able to convergence
    // based on sync mechanism
    fn run(&self, nodes: &mut Vec<Node>) {
        const PER_FETCH_BLOCK_LIMIT: usize = 128;

        exit_ibd_mode(nodes);
        let test_node = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];

        // test_node == node1 == chain1, height = 139 = PER_FETCH_BLOCK_LIMIT + 10 + 1
        node1.generate_blocks(PER_FETCH_BLOCK_LIMIT + 10);
        test_node.connect(node1);
        test_node.waiting_for_sync(node1, PER_FETCH_BLOCK_LIMIT as u64 + 10 + 1);
        test_node.disconnect(node1);

        // test_node == node2 == chain2, height = 149 = PER_FETCH_BLOCK_LIMIT + 20 + 1
        node2.generate_blocks(PER_FETCH_BLOCK_LIMIT + 20);
        test_node.connect(node2);
        test_node.waiting_for_sync(node2, PER_FETCH_BLOCK_LIMIT as u64 + 20 + 1);
        test_node.disconnect(node2);

        // test_node == node1 == chain1, height = 169 = PER_FETCH_BLOCK_LIMIT + 10 + 30 + 1
        node1.generate_blocks(30);
        test_node.connect(node1);
        test_node.waiting_for_sync(node1, PER_FETCH_BLOCK_LIMIT as u64 + 10 + 30 + 1);
    }
}

pub struct ForksContainSameTransactions;

impl Spec for ForksContainSameTransactions {
    crate::setup!(num_nodes: 4);

    // Case:
    //   1. 3 forks `chain0`, `chain1` and `chain2`
    //   2. `chain0` and `chain1` both contain transaction `tx`, but `chain2` not
    //   3. Initialize node holds `chain0` as the main chain, then switch to `chain2`, finally to
    //      `chain1`. We expect `get_transaction(tx)` returns successfully.
    fn run(&self, nodes: &mut Vec<Node>) {
        exit_ibd_mode(nodes);
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];
        let target_node = &nodes[3];

        node0.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);

        let transaction = node0.new_transaction_spend_tip_cellbase();

        // Build `chain0`, contain the target `transaction`, with length = 41
        {
            node0.generate_blocks(20);
            node0.submit_transaction(&transaction);
            node0.generate_blocks(20);
        }

        // Build `chain1`, contain the target `transaction`, with length = 61
        {
            // `sleep` to make sure that the chain1[2] != chain2[2]
            sleep(Duration::from_millis(1));
            node1.generate_blocks(30);
            node1.submit_transaction(&transaction);
            node1.generate_blocks(40);
        }

        // Build `chain2`, all the blocks are empty, with length = 51
        {
            sleep(Duration::from_millis(1));
            node2.generate_blocks(60);
        }

        let (rpc_client0, rpc_client1, rpc_client2) =
            (node0.rpc_client(), node1.rpc_client(), node2.rpc_client());
        let header0 = rpc_client0.get_header_by_number(2).unwrap();
        let header1 = rpc_client1.get_header_by_number(2).unwrap();
        let header2 = rpc_client2.get_header_by_number(2).unwrap();

        assert_ne!(header0.hash, header1.hash);
        assert_ne!(header0.hash, header2.hash);
        assert_ne!(header1.hash, header2.hash);

        // `target_node` holds `chain0` as the main chain
        target_node.connect(node0);
        target_node.waiting_for_sync(node0, DEFAULT_TX_PROPOSAL_WINDOW.1 + 43);
        target_node.disconnect(node0);
        is_transaction_committed(target_node, &transaction);

        // `target_node` switch to `chain2` as the main chain
        target_node.connect(node2);
        target_node.waiting_for_sync(node2, 61);
        target_node.disconnect(node2);
        is_transaction_committed(target_node, &transaction);

        // `target_node` switch to `chain1` as the main chain
        target_node.connect(node1);
        target_node.waiting_for_sync(node1, 71);
        target_node.disconnect(node1);
        is_transaction_committed(target_node, &transaction);
    }
}

pub struct ForksContainSameUncle;

impl Spec for ForksContainSameUncle {
    crate::setup!(num_nodes: 2);

    // Case: Two nodes maintain two different forks, but contains a same uncle block, should be
    //       able to sync with each other.
    //
    // Consider the forks-graph: fork-A add block-U as uncle into block-A, fork-B add block-U
    // as uncle into block-B as well. We expect that different nodes maintains fork-A and fork-B
    // can sync with each other.
    //
    //                     /-> A(U)
    // genesis -> 1 -> 2 ->
    //             \       \-> B(U)
    //              \-> U
    //
    fn run(&self, nodes: &mut Vec<Node>) {
        let node_a = &nodes[0];
        let node_b = &nodes[1];
        exit_ibd_mode(nodes);

        info!("(1) Construct an uncle before fork point");
        let uncle = node_a.construct_uncle();
        node_a.generate_block();
        node_b.generate_block();

        info!("(2) Add `uncle` into different forks in node_a and node_b");
        node_a.submit_block(&uncle);
        node_b.submit_block(&uncle);
        let block_a = node_a
            .new_block_builder(None, None, None)
            .set_uncles(vec![uncle.as_uncle()])
            .build();
        let block_b = node_b
            .new_block_builder(None, None, None)
            .set_uncles(vec![uncle.as_uncle()])
            .timestamp((block_a.timestamp() + 2).pack())
            .build();
        node_a.submit_block(&block_a);
        node_b.submit_block(&block_b);

        info!("(3) Make node_b's fork longer(to help check whether is synchronized)");
        node_b.generate_block();

        info!("(4) Connect node_a and node_b, expect that they sync into convergence");
        node_a.connect(node_b);
        waiting_for_sync(nodes);
    }
}

pub struct ForkedTransaction;

impl Spec for ForkedTransaction {
    crate::setup!(num_nodes: 2);

    // Case: Check TxStatus of transaction on main-fork, verified-fork and unverified-fork
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let finalization_delay_length = node0.consensus().finalization_delay_length();
        (0..=finalization_delay_length).for_each(|_| {
            let block = node0.new_block(None, None, None);
            node0.submit_block(&block);
            node1.submit_block(&block);
        });

        exit_ibd_mode(nodes);
        let fixed_point = node0.get_tip_block_number();
        let tx = node1.new_transaction_spend_tip_cellbase();

        // `node0` doesn't have `tx`      => TxStatus: None
        {
            node0.generate_blocks(1 + 2 * finalization_delay_length as usize);
            let tx_status = node0.rpc_client().get_transaction(tx.hash());
            assert!(tx_status.is_none(), "node0 maintains tx in unverified fork");
        }

        // `node1` have `tx` on main-fork => TxStatus: Some(Committed)
        {
            node1.submit_transaction(&tx);
            node1.generate_blocks(2 * finalization_delay_length as usize);
            assert!(is_transaction_committed(node1, &tx));
        }

        // `node0` have `tx` on unverified-fork only => TxStatus: None
        //
        // We submit the main-fork of `node1` to `node0`, that will be persisted as an
        // unverified-fork inside `node0`.
        {
            (fixed_point..=node1.get_tip_block_number()).for_each(|number| {
                let block = node1.get_block_by_number(number);
                node0.submit_block(&block);
            });
            let tx_status = node0.rpc_client().get_transaction(tx.hash());
            assert!(tx_status.is_none(), "node0 maintains tx in unverified fork");
        }

        // node1 have `tx` on verified-fork   => TxStatus: Some(Pending)
        //
        // We submit the main-fork of `node0` to `node1`, that will trigger switching forks. Then
        // the original main-fork of `node0` will become side verified-fork. And `tx` will be moved
        // to gap-transactions-pool during switching forks
        {
            (fixed_point..=node0.get_tip_block_number()).for_each(|number| {
                let block = node0.get_block_by_number(number);
                node1.submit_block(&block);
            });

            assert!(is_transaction_pending(node1, &tx,));
        }
    }
}

fn modify_block_transaction<F>(
    block: BlockView,
    transaction_index: usize,
    modify_transaction: F,
) -> BlockView
where
    F: FnOnce(TransactionView) -> TransactionView,
{
    let mut transactions = block.transactions();
    transactions[transaction_index] = modify_transaction(transactions[transaction_index].clone());
    block
        .as_advanced_builder()
        .set_transactions(transactions)
        .build()
}
