use crate::utils::{build_relay_tx_hashes, build_relay_txs, sleep, wait_until};
use crate::{Net, Spec, TestProtocol, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_sync::{NetworkProtocol, RETRY_ASK_TX_TIMEOUT_INCREASE};
use ckb_types::{
    core::{Capacity, TransactionBuilder},
    packed::{CellInput, GetRelayTransactions, OutPoint, RelayMessage},
    prelude::*,
};
use log::info;
use std::time::Duration;

pub struct TransactionRelayBasic;

impl Spec for TransactionRelayBasic {
    crate::name!("transaction_relay_basic");

    crate::setup!(num_nodes: 3);

    fn run(&self, net: &mut Net) {
        net.exit_ibd_mode();

        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];
        let node2 = &net.nodes[2];

        info!("Generate new transaction on node1");
        node1.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);
        let hash = node1.generate_transaction();

        info!("Waiting for relay");
        let rpc_client = node0.rpc_client();
        let ret = wait_until(10, || {
            if let Some(transaction) = rpc_client.get_transaction(hash.clone()) {
                transaction.tx_status.block_hash.is_none()
            } else {
                false
            }
        });
        assert!(ret, "Transaction should be relayed to node0");

        let rpc_client = node2.rpc_client();
        let ret = wait_until(10, || {
            if let Some(transaction) = rpc_client.get_transaction(hash.clone()) {
                transaction.tx_status.block_hash.is_none()
            } else {
                false
            }
        });
        assert!(ret, "Transaction should be relayed to node2");
    }
}

const MIN_CAPACITY: u64 = 61_0000_0000;

pub struct TransactionRelayMultiple;

impl Spec for TransactionRelayMultiple {
    crate::name!("transaction_relay_multiple");

    crate::setup!(num_nodes: 5);

    fn run(&self, net: &mut Net) {
        let node0 = &net.nodes[0];
        (0..node0.consensus().tx_proposal_window().farthest() + 2).for_each(|_| {
            net.exit_ibd_mode();
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
        net.waiting_for_sync(node0.get_tip_block_number());

        info!("Send multiple transactions to node0");
        let tx_hash = transaction.hash().to_owned();
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
                    .output(output.clone())
                    .input(CellInput::new(OutPoint::new(tx_hash.clone(), i as u32), 0))
                    .output_data(Default::default())
                    .build();
                node0.rpc_client().send_transaction(tx.data().into());
            });

        node0.generate_block();
        node0.generate_block();
        node0.generate_block();
        net.waiting_for_sync(node0.get_tip_block_number());

        info!("All transactions should be relayed and mined");
        node0.assert_tx_pool_size(0, 0);

        net.nodes.iter().for_each(|node| {
            assert_eq!(
                node.get_tip_block().transactions().len() as u64,
                txs_num + 1
            )
        });
    }
}

pub struct TransactionRelayTimeout;

impl Spec for TransactionRelayTimeout {
    crate::name!("get_relay_transaction_timeout");

    crate::setup!(
        connect_all: false,
        num_nodes: 1,
        protocols: vec![TestProtocol::relay(), TestProtocol::sync()],
    );

    fn run(&self, net: &mut Net) {
        let node = net.nodes.pop().unwrap();
        node.generate_blocks(4);
        net.connect(&node);
        let (pi, _, _) = net.receive();
        let dummy_tx = TransactionBuilder::default().build();
        info!("Sending RelayTransactionHashes to node");
        net.send(
            NetworkProtocol::RELAY.into(),
            pi,
            build_relay_tx_hashes(&[dummy_tx.hash()]),
        );
        info!("Receiving GetRelayTransactions message from node");
        assert!(
            wait_get_relay_txs(&net),
            "timeout to wait GetRelayTransactions"
        );

        let wait_seconds = RETRY_ASK_TX_TIMEOUT_INCREASE.as_secs();
        info!("Waiting for {} seconds", wait_seconds);
        // Relay protocol will retry 30 seconds later when same GetRelayTransactions received from other peer
        // (not happend in current test case)
        sleep(wait_seconds);
        assert!(
            !wait_get_relay_txs(&net),
            "should not receive GetRelayTransactions again"
        );
    }
}

pub struct RelayInvalidTransaction;

impl Spec for RelayInvalidTransaction {
    crate::name!("relay_invalid_transaction");

    crate::setup!(
        connect_all: false,
        num_nodes: 1,
        protocols: vec![TestProtocol::relay(), TestProtocol::sync()],
        retry_failed: 1,
    );

    fn run(&self, net: &mut Net) {
        let node = net.nodes.pop().unwrap();
        node.generate_blocks(4);
        net.connect(&node);
        let (pi, _, _) = net.receive();
        let dummy_tx = TransactionBuilder::default().build();
        info!("Sending RelayTransactionHashes to node");
        net.send(
            NetworkProtocol::RELAY.into(),
            pi,
            build_relay_tx_hashes(&[dummy_tx.hash()]),
        );
        info!("Receiving GetRelayTransactions message from node");
        assert!(
            wait_get_relay_txs(&net),
            "timeout to wait GetRelayTransactions"
        );

        assert!(
            node.rpc_client().get_banned_addresses().is_empty(),
            "Banned addresses list should empty"
        );
        info!("Sending RelayTransactions to node");
        net.send(
            NetworkProtocol::RELAY.into(),
            pi,
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

fn wait_get_relay_txs(net: &Net) -> bool {
    wait_until(10, || {
        if let Ok((_, _, data)) = net.receive_timeout(Duration::from_secs(10)) {
            if let Ok(message) = RelayMessage::from_slice(&data) {
                return message.to_enum().item_name() == GetRelayTransactions::NAME;
            }
        }
        false
    })
}
