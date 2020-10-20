use crate::node::{connect_all, exit_ibd_mode, waiting_for_sync};
use crate::utils::{build_relay_tx_hashes, build_relay_txs, sleep, wait_until};
use crate::{Net, Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_network::SupportProtocols;
use ckb_sync::RETRY_ASK_TX_TIMEOUT_INCREASE;
use ckb_types::{
    core::{Capacity, TransactionBuilder},
    packed::{CellInput, GetRelayTransactions, OutPoint, RelayMessage},
    prelude::*,
};
use log::info;

pub struct TransactionRelayBasic;

impl Spec for TransactionRelayBasic {
    crate::setup!(num_nodes: 3);

    fn run(&self, nodes: &mut Vec<Node>) {
        connect_all(nodes);
        exit_ibd_mode(nodes);

        nodes[0].generate_blocks_until_contains_valid_cellbase();
        let hash = nodes[0].generate_transaction();
        let relayed = wait_until(10, || {
            nodes
                .iter()
                .all(|node| node.rpc_client().get_transaction(hash.clone()).is_some())
        });
        assert!(
            relayed,
            "Transaction should be relayed from node0 to others"
        );
    }
}

const MIN_CAPACITY: u64 = 61_0000_0000;

pub struct TransactionRelayMultiple;

impl Spec for TransactionRelayMultiple {
    crate::setup!(num_nodes: 5);

    fn run(&self, nodes: &mut Vec<Node>) {
        connect_all(nodes);

        let node0 = &nodes[0];
        (0..node0.consensus().tx_proposal_window().farthest() + 2).for_each(|_| {
            exit_ibd_mode(nodes);
        });

        info!("Use generated block's cellbase as tx input");
        let block = node0.get_tip_block();
        let reward: Capacity = block.transactions()[0]
            .outputs()
            .as_reader()
            .get(0)
            .unwrap()
            .to_entity()
            .capacity()
            .unpack();
        let txs_num = reward.as_u64() / MIN_CAPACITY;

        let parent_hash = block.transactions()[0].hash();
        let temp_transaction = node0.new_transaction(parent_hash);
        let output = temp_transaction
            .outputs()
            .as_reader()
            .get(0)
            .unwrap()
            .to_entity()
            .as_builder()
            .capacity(Capacity::shannons(reward.as_u64() / txs_num).pack())
            .build();
        let mut tb = temp_transaction
            .as_advanced_builder()
            .set_outputs(Vec::new())
            .set_outputs_data(Vec::new());
        for _ in 0..txs_num {
            tb = tb.output(output.clone()).output_data(Default::default());
        }
        let transaction = tb.build();
        node0
            .rpc_client()
            .send_transaction(transaction.data().into());
        node0.generate_block();
        node0.generate_block();
        node0.generate_block();
        waiting_for_sync(nodes);

        info!("Send multiple transactions to node0");
        let tx_hash = transaction.hash();
        transaction
            .outputs()
            .into_iter()
            .enumerate()
            .for_each(|(i, output)| {
                let tx = TransactionBuilder::default()
                    .cell_dep(
                        transaction
                            .cell_deps()
                            .as_reader()
                            .get(0)
                            .unwrap()
                            .to_entity(),
                    )
                    .output(output)
                    .input(CellInput::new(OutPoint::new(tx_hash.clone(), i as u32), 0))
                    .output_data(Default::default())
                    .build();
                node0.rpc_client().send_transaction(tx.data().into());
            });

        node0.generate_block();
        node0.generate_block();
        node0.generate_block();
        waiting_for_sync(nodes);

        info!("All transactions should be relayed and mined");
        node0.assert_tx_pool_size(0, 0);

        nodes.iter().for_each(|node| {
            assert_eq!(
                node.get_tip_block().transactions().len() as u64,
                txs_num + 1
            )
        });
    }
}

pub struct TransactionRelayTimeout;

impl Spec for TransactionRelayTimeout {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = nodes.pop().unwrap();
        node.generate_blocks(4);
        let mut net = Net::new(
            self.name(),
            node.consensus(),
            vec![SupportProtocols::Sync, SupportProtocols::Relay],
        );
        net.connect(&node);
        let dummy_tx = TransactionBuilder::default().build();
        info!("Sending RelayTransactionHashes to node");
        net.send(
            &node,
            SupportProtocols::Relay,
            build_relay_tx_hashes(&[dummy_tx.hash()]),
        );
        info!("Receiving GetRelayTransactions message from node");
        assert!(
            wait_get_relay_txs(&net, &node),
            "timeout to wait GetRelayTransactions"
        );

        let wait_seconds = RETRY_ASK_TX_TIMEOUT_INCREASE.as_secs();
        info!("Waiting for {} seconds", wait_seconds);
        // Relay protocol will retry 30 seconds later when same GetRelayTransactions received from other peer
        // (not happened in current test case)
        sleep(wait_seconds);
        assert!(
            !wait_get_relay_txs(&net, &node),
            "should not receive GetRelayTransactions again"
        );
    }
}

pub struct RelayInvalidTransaction;

impl Spec for RelayInvalidTransaction {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes.pop().unwrap();
        node.generate_blocks(4);
        let mut net = Net::new(
            self.name(),
            node.consensus(),
            vec![SupportProtocols::Sync, SupportProtocols::Relay],
        );
        net.connect(node);
        let dummy_tx = TransactionBuilder::default().build();
        info!("Sending RelayTransactionHashes to node");
        net.send(
            node,
            SupportProtocols::Relay,
            build_relay_tx_hashes(&[dummy_tx.hash()]),
        );
        info!("Receiving GetRelayTransactions message from node");
        assert!(
            wait_get_relay_txs(&net, node),
            "timeout to wait GetRelayTransactions"
        );

        assert!(
            node.rpc_client().get_banned_addresses().is_empty(),
            "Banned addresses list should empty"
        );
        info!("Sending RelayTransactions to node");
        net.send(
            &node,
            SupportProtocols::Relay,
            build_relay_txs(&[(dummy_tx, 333)]),
        );

        wait_until(20, || node.rpc_client().get_banned_addresses().len() == 1);
        let banned_addrs = node.rpc_client().get_banned_addresses();
        assert_eq!(
            banned_addrs.len(),
            1,
            "Net should be banned: {:?}",
            banned_addrs
        );
    }
}

fn wait_get_relay_txs(net: &Net, node: &Node) -> bool {
    net.should_receive(node, |data| {
        RelayMessage::from_slice(data)
            .map(|message| message.to_enum().item_name() == GetRelayTransactions::NAME)
            .unwrap_or(false)
    })
}

pub struct TransactionRelayEmptyPeers;

impl Spec for TransactionRelayEmptyPeers {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        exit_ibd_mode(nodes);

        let node0 = &nodes[0];
        let node1 = &nodes[1];

        node0.connect(node1);
        node0.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);
        node0.waiting_for_sync(node1, DEFAULT_TX_PROPOSAL_WINDOW.1 + 3);
        info!("Disconnect node1 and generate new transaction on node0");
        node0.disconnect(&node1);
        let hash = node0.generate_transaction();

        info!("Transaction should be relayed to node1 when node0's peers become none-empty");
        node0.connect(node1);
        let rpc_client = node1.rpc_client();
        let ret = wait_until(10, || {
            if let Some(transaction) = rpc_client.get_transaction(hash.clone()) {
                transaction.tx_status.block_hash.is_none()
            } else {
                false
            }
        });
        assert!(ret, "Transaction should be relayed to node1");
    }
}
