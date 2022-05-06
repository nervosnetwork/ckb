use crate::node::{connect_all, waiting_for_sync};
use crate::util::cell::gen_spendable;
use crate::util::mining::out_ibd_mode;
use crate::util::transaction::{always_success_transaction, always_success_transactions};
use crate::utils::{build_relay_tx_hashes, build_relay_txs, sleep, wait_until};
use crate::{Net, Node, Spec};
use ckb_constant::sync::RETRY_ASK_TX_TIMEOUT_INCREASE;
use ckb_logger::info;
use ckb_network::SupportProtocols;
use ckb_types::{
    core::TransactionBuilder,
    packed::{GetRelayTransactions, RelayMessage},
    prelude::*,
};

pub struct TransactionRelayBasic;

impl Spec for TransactionRelayBasic {
    crate::setup!(num_nodes: 3);

    fn run(&self, nodes: &mut Vec<Node>) {
        out_ibd_mode(nodes);
        connect_all(nodes);

        let node1 = &nodes[1];
        let cells = gen_spendable(node1, 1);
        let transaction = always_success_transaction(node1, &cells[0]);
        let hash = node1.submit_transaction(&transaction);

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

pub struct TransactionRelayMultiple;

impl Spec for TransactionRelayMultiple {
    crate::setup!(num_nodes: 5);

    fn run(&self, nodes: &mut Vec<Node>) {
        connect_all(nodes);

        let node0 = &nodes[0];
        let cells = gen_spendable(node0, 10);
        let transactions = always_success_transactions(node0, &cells);
        transactions.iter().for_each(|tx| {
            node0.submit_transaction(tx);
        });

        let relayed = wait_until(20, || {
            nodes.iter().all(|node| {
                transactions
                    .iter()
                    .all(|tx| node.rpc_client().get_transaction(tx.hash()).is_some())
            })
        });
        assert!(relayed, "all transactions should be relayed");

        node0.mine_until_transactions_confirm();
        waiting_for_sync(nodes);
        nodes.iter().for_each(|node| {
            node.assert_tx_pool_size(0, 0);
            assert_eq!(
                node.get_tip_block().transactions().len(),
                transactions.len() + 1
            )
        });
    }
}

pub struct TransactionRelayTimeout;

impl Spec for TransactionRelayTimeout {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = nodes.pop().unwrap();
        node.mine(4);
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
        node.mine(4);
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
            node,
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
        out_ibd_mode(nodes);

        let node0 = &nodes[0];
        let node1 = &nodes[1];

        let cells = gen_spendable(node0, 1);
        let transaction = always_success_transaction(node1, &cells[0]);

        // Connect to node1 and then disconnect
        node0.connect(node1);
        waiting_for_sync(&[node0, node1]);
        node0.disconnect(node1);

        // Submit transaction. Node0 has empty peers at present.
        node0.submit_transaction(&transaction);

        info!("Transaction should be relayed to node1 when node0's peers become none-empty");
        node0.connect(node1);
        let relayed = wait_until(10, || {
            node1
                .rpc_client()
                .get_transaction(transaction.hash())
                .is_some()
        });
        assert!(relayed, "Transaction should be relayed to node1");
    }
}
