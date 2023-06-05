use crate::node::{connect_all, waiting_for_sync};
use crate::util::cell::gen_spendable;
use crate::util::mining::out_ibd_mode;
use crate::util::transaction::{always_success_transaction, always_success_transactions};
use crate::utils::{build_relay_tx_hashes, build_relay_txs, sleep, wait_until};
use crate::{Net, Node, Spec};
use ckb_constant::sync::RETRY_ASK_TX_TIMEOUT_INCREASE;
use ckb_jsonrpc_types::Status;
use ckb_logger::info;
use ckb_network::SupportProtocols;
use ckb_types::{
    core::{capacity_bytes, Capacity, TransactionBuilder},
    packed::{CellOutputBuilder, GetRelayTransactions, RelayMessage},
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
            nodes.iter().all(|node| {
                node.rpc_client().get_transaction(hash.clone()).cycles == Some(537.into())
            })
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
                transactions.iter().all(|tx| {
                    node.rpc_client()
                        .get_transaction(tx.hash())
                        .transaction
                        .is_some()
                })
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
            vec![SupportProtocols::Sync, SupportProtocols::RelayV3],
        );
        net.connect(&node);
        let dummy_tx = TransactionBuilder::default().build();
        info!("Sending RelayTransactionHashes to node");
        net.send(
            &node,
            SupportProtocols::RelayV3,
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
            vec![SupportProtocols::Sync, SupportProtocols::RelayV3],
        );
        net.connect(node);
        let dummy_tx = TransactionBuilder::default().build();
        info!("Sending RelayTransactionHashes to node");
        net.send(
            node,
            SupportProtocols::RelayV3,
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
            SupportProtocols::RelayV3,
            build_relay_txs(&[(dummy_tx, 333)]),
        );

        wait_until(20, || node.rpc_client().get_banned_addresses().len() == 1);
        let banned_addrs = node.rpc_client().get_banned_addresses();
        assert_eq!(
            banned_addrs.len(),
            1,
            "Net should be banned: {banned_addrs:?}"
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
                .transaction
                .is_some()
        });
        assert!(relayed, "Transaction should be relayed to node1");
    }
}

pub struct TransactionRelayConflict;

impl Spec for TransactionRelayConflict {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        out_ibd_mode(nodes);
        connect_all(nodes);

        let node0 = &nodes[0];
        let node1 = &nodes[1];

        node0.mine_until_out_bootstrap_period();
        waiting_for_sync(nodes);

        let tx_hash_0 = node0.generate_transaction();
        info!("Generate 2 txs with same input");
        let tx1 = node0.new_transaction(tx_hash_0.clone());
        let tx2_temp = node0.new_transaction(tx_hash_0);
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(80).pack())
            .build();

        let tx2 = tx2_temp
            .as_advanced_builder()
            .set_outputs(vec![output])
            .build();
        node0.rpc_client().send_transaction(tx1.data().into());
        sleep(6);

        let res = node0.rpc_client().get_transaction(tx1.hash());
        assert!(matches!(res.tx_status.status, Status::Pending));

        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        assert!(res.is_err());
        assert!(res
            .err()
            .unwrap()
            .to_string()
            .contains("TransactionFailedToResolve: Resolve failed Dead"));

        let relayed = wait_until(20, || {
            [tx1.hash()].iter().all(|hash| {
                node1
                    .rpc_client()
                    .get_transaction(hash.clone())
                    .transaction
                    .is_some()
            })
        });
        assert!(relayed, "all transactions should be relayed");

        let proposed = node1.mine_with_blocking(|template| template.proposals.len() != 2);
        node1.mine_with_blocking(|template| template.number.value() != (proposed + 1));

        waiting_for_sync(nodes);
        node0.wait_for_tx_pool();
        node1.wait_for_tx_pool();

        let ret = node1
            .rpc_client()
            .get_transaction_with_verbosity(tx2.hash(), 1);
        assert!(matches!(ret.tx_status.status, Status::Unknown));

        node0.remove_transaction(tx1.hash());
        node0.remove_transaction(tx2.hash());
        node1.remove_transaction(tx1.hash());
        node1.remove_transaction(tx2.hash());
        node0.wait_for_tx_pool();
        node1.wait_for_tx_pool();

        let result = wait_until(5, || {
            let tx_pool_info = node0.get_tip_tx_pool_info();
            tx_pool_info.orphan.value() == 0 && tx_pool_info.pending.value() == 0
        });
        assert!(result, "remove txs from node0");
        let result = wait_until(5, || {
            let tx_pool_info = node1.get_tip_tx_pool_info();
            tx_pool_info.orphan.value() == 0 && tx_pool_info.pending.value() == 0
        });
        assert!(result, "remove txs from node1");

        let relayed = wait_until(10, || {
            // re-broadcast
            // TODO: (yukang) double comfirm this behavior
            let _ = node1
                .rpc_client()
                .send_transaction_result(tx2.data().into());

            node0
                .rpc_client()
                .get_transaction(tx2.hash())
                .transaction
                .is_some()
        });
        assert!(relayed, "Transaction should be relayed to node1");
    }
}
