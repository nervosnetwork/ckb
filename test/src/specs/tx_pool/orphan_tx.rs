use crate::util::transaction::{relay_tx, send_tx};
use crate::utils::wait_until;
use crate::{Net, Node, Spec};
use ckb_jsonrpc_types::Status;
use ckb_network::SupportProtocols;
use ckb_types::packed::CellOutputBuilder;
use ckb_types::{
    bytes::Bytes,
    core::{capacity_bytes, Capacity, TransactionBuilder, TransactionView},
    packed,
};
use ckb_types::{
    packed::{CellInput, OutPoint},
    prelude::*,
};

const ALWAYS_SUCCESS_SCRIPT_CYCLE: u64 = 537;
// always_failure, as the name implies, so it doesn't matter what the cycles are
const ALWAYS_FAILURE_SCRIPT_CYCLE: u64 = 1000;

pub struct OrphanTxAccepted;

impl Spec for OrphanTxAccepted {
    crate::setup!(num_nodes: 1);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &mut nodes[0];
        node0.mine_until_out_bootstrap_period();

        let mut net = Net::new(
            self.name(),
            node0.consensus(),
            vec![SupportProtocols::RelayV3],
        );
        net.connect(node0);

        let parent_tx = node0.new_transaction_spend_tip_cellbase();
        let child_tx = node0.new_transaction(parent_tx.hash());

        relay_tx(&net, node0, child_tx, ALWAYS_SUCCESS_SCRIPT_CYCLE);
        let result = wait_until(5, || {
            let tx_pool_info = node0.get_tip_tx_pool_info();
            tx_pool_info.orphan.value() == 1 && tx_pool_info.pending.value() == 0
        });
        assert!(
            result,
            "Send child tx first, it will be added to orphan tx pool"
        );

        relay_tx(&net, node0, parent_tx, ALWAYS_SUCCESS_SCRIPT_CYCLE);
        let result = wait_until(5, || {
            let tx_pool_info = node0.get_tip_tx_pool_info();
            tx_pool_info.orphan.value() == 0 && tx_pool_info.pending.value() == 2
        });
        assert!(
            result,
            "Send parent tx, the child tx will be moved from orphan tx pool to pending tx pool"
        );
    }
}

pub struct OrphanTxRejected;

impl Spec for OrphanTxRejected {
    crate::setup!(num_nodes: 1);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &mut nodes[0];
        node0.mine_until_out_bootstrap_period();

        let mut net = Net::new(
            self.name(),
            node0.consensus(),
            vec![SupportProtocols::RelayV3],
        );
        net.connect(node0);

        let parent_tx = node0.new_transaction_spend_tip_cellbase();
        let child_tx = node0.new_always_failure_transaction(parent_tx.hash());
        let child_hash = child_tx.hash();

        relay_tx(&net, node0, child_tx, ALWAYS_FAILURE_SCRIPT_CYCLE);
        let result = wait_until(5, || {
            let tx_pool_info = node0.get_tip_tx_pool_info();
            tx_pool_info.orphan.value() == 1 && tx_pool_info.pending.value() == 0
        });
        assert!(
            result,
            "Send child tx first, it will be added to orphan tx pool"
        );

        relay_tx(&net, node0, parent_tx, ALWAYS_SUCCESS_SCRIPT_CYCLE);
        let result = wait_until(5, || {
            let tx_pool_info = node0.get_tip_tx_pool_info();
            tx_pool_info.orphan.value() == 0 && tx_pool_info.pending.value() == 1
        });
        assert!(
            result,
            "Send parent tx, the child tx will be moved from orphan tx pool because of always_failure"
        );
        wait_until(20, || node0.rpc_client().get_banned_addresses().len() == 1);

        let ret = node0
            .rpc_client()
            .get_transaction_with_verbosity(child_hash, 2);
        assert!(ret.transaction.is_none());
        assert!(matches!(ret.tx_status.status, Status::Rejected));
    }
}

// construct a tx chain with such structure:
//
//               parent
//                 |
//                tx1
//              /  |  \
//           tx11 tx12 tx13
//             \   |   /
//              final_tx
//
fn build_tx_chain(
    node0: &Node,
) -> (
    Net,
    (
        TransactionView,
        TransactionView,
        TransactionView,
        TransactionView,
        TransactionView,
        TransactionView,
    ),
) {
    node0.mine_until_out_bootstrap_period();
    let parent = node0.new_transaction_with_capacity(capacity_bytes!(800));

    let script = node0.always_success_script();
    let new_output1 = CellOutputBuilder::default()
        .capacity(capacity_bytes!(200).into())
        .lock(script.clone())
        .build();
    let new_output2 = new_output1.clone();
    let new_output3 = new_output1.clone();

    let tx1 = parent
        .as_advanced_builder()
        .set_inputs(vec![CellInput::new(OutPoint::new(parent.hash(), 0), 0)])
        .set_outputs(vec![new_output1, new_output2, new_output3])
        .set_outputs_data(vec![Default::default(); 3])
        .build();

    let tx11 =
        node0.new_transaction_with_capacity_and_index(tx1.hash(), capacity_bytes!(100), 0, 0);
    let tx12 =
        node0.new_transaction_with_capacity_and_index(tx1.hash(), capacity_bytes!(100), 1, 0);
    let tx13 =
        node0.new_transaction_with_capacity_and_index(tx1.hash(), capacity_bytes!(100), 2, 0);

    let cell_dep = node0.always_success_cell_dep();
    let final_output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(80).into())
        .lock(script)
        .build();
    let final_tx = TransactionBuilder::default()
        .cell_dep(cell_dep)
        .set_inputs(vec![
            CellInput::new(OutPoint::new(tx11.hash(), 0), 0),
            CellInput::new(OutPoint::new(tx12.hash(), 0), 0),
            CellInput::new(OutPoint::new(tx13.hash(), 0), 0),
        ])
        .set_outputs(vec![final_output])
        .set_outputs_data(vec![Default::default(); 1])
        .build();

    let mut net = Net::new(
        "orphan_tx_test",
        node0.consensus(),
        vec![SupportProtocols::RelayV3],
    );
    net.connect(node0);

    (net, (parent, tx1, tx11, tx12, tx13, final_tx))
}

fn run_replay_tx(
    net: &Net,
    node0: &Node,
    tx: TransactionView,
    orphan_tx_cnt: u64,
    pending_cnt: u64,
) -> bool {
    relay_tx(net, node0, tx, ALWAYS_SUCCESS_SCRIPT_CYCLE);

    wait_until(5, || {
        let tx_pool_info = node0.get_tip_tx_pool_info();
        tx_pool_info.orphan.value() == orphan_tx_cnt && tx_pool_info.pending.value() == pending_cnt
    })
}

fn run_send_tx(
    net: &Net,
    node0: &Node,
    tx: TransactionView,
    orphan_tx_cnt: u64,
    pending_cnt: u64,
) -> bool {
    send_tx(net, node0, tx, ALWAYS_SUCCESS_SCRIPT_CYCLE);

    wait_until(5, || {
        let tx_pool_info = node0.get_tip_tx_pool_info();
        tx_pool_info.orphan.value() == orphan_tx_cnt && tx_pool_info.pending.value() == pending_cnt
    })
}

fn should_receive_get_relay_transactions(net: &Net, node0: &Node, assert_message: &str) {
    let ret = net.should_receive(node0, |data: &Bytes| {
        packed::RelayMessage::from_slice(data)
            .map(|message| message.to_enum().item_name() == packed::GetRelayTransactions::NAME)
            .unwrap_or(false)
    });
    assert!(ret, "{}", assert_message);
}

pub struct TxPoolOrphanNormal;
impl Spec for TxPoolOrphanNormal {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let (net, (parent, tx1, tx11, tx12, tx13, final_tx)) = build_tx_chain(node0);

        assert!(
            run_replay_tx(&net, node0, parent, 0, 1),
            "parent sended expect nothing in orphan pool"
        );
        assert!(
            run_replay_tx(&net, node0, tx1, 0, 2),
            "tx1 is sent expect nothing in orphan pool"
        );
        assert!(
            run_replay_tx(&net, node0, tx11, 0, 3),
            "tx11 is sent expect nothing in orphan pool"
        );
        assert!(
            run_replay_tx(&net, node0, tx12, 0, 4),
            "tx12 is sent expect nothing in orphan pool"
        );
        assert!(
            run_replay_tx(&net, node0, tx13, 0, 5),
            "tx13 is sent expect nothing in orphan pool"
        );
        assert!(
            run_replay_tx(&net, node0, final_tx, 0, 6),
            "final_tx is sent expect nothing in orphan pool"
        );
    }
}

pub struct TxPoolOrphanReverse;
impl Spec for TxPoolOrphanReverse {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let (net, (parent, tx1, tx11, tx12, tx13, final_tx)) = build_tx_chain(node0);

        assert!(
            run_replay_tx(&net, node0, final_tx, 1, 0),
            "expect final_tx is in orphan pool"
        );
        should_receive_get_relay_transactions(&net, node0, "node should ask for tx11 tx12 tx13");

        assert!(run_send_tx(&net, node0, tx13, 2, 0), "tx13 in orphan pool");
        should_receive_get_relay_transactions(&net, node0, "node should ask for tx1");

        assert!(
            run_send_tx(&net, node0, tx12, 3, 0),
            "tx12 is in orphan pool"
        );
        assert!(run_send_tx(&net, node0, tx11, 4, 0), "tx11 is in orphan");
        assert!(run_send_tx(&net, node0, tx1, 5, 0), "tx1 is in orphan");

        should_receive_get_relay_transactions(&net, node0, "node should ask for parent");
        assert!(run_send_tx(&net, node0, parent, 0, 6), "all is in pending");
    }
}

pub struct TxPoolOrphanUnordered;
impl Spec for TxPoolOrphanUnordered {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let (net, (parent, tx1, tx11, tx12, tx13, final_tx)) = build_tx_chain(node0);

        assert!(
            run_replay_tx(&net, node0, final_tx, 1, 0),
            "expect final_tx is in orphan pool"
        );

        should_receive_get_relay_transactions(&net, node0, "node should ask for tx11 tx12 tx13");

        assert!(run_send_tx(&net, node0, tx11, 2, 0), "tx11 in orphan pool");
        should_receive_get_relay_transactions(&net, node0, "node should ask for tx1");

        let tx12_clone = tx12.clone();
        assert!(
            run_send_tx(&net, node0, tx12, 3, 0),
            "tx12 is in orphan pool"
        );

        // set tx12_clone with rpc
        let ret = node0
            .rpc_client()
            .send_transaction_result(tx12_clone.data().into());
        assert!(ret
            .err()
            .unwrap()
            .to_string()
            .contains("already exists in transaction_pool"));

        assert!(
            run_replay_tx(&net, node0, parent, 3, 1),
            "parent is sent, should be in pending without change orphan pool"
        );
        assert!(
            run_send_tx(&net, node0, tx1, 1, 4),
            "tx1 is sent, orphan pool only contains final_tx"
        );

        assert!(
            run_send_tx(&net, node0, tx13, 0, 6),
            "tx13 is sent, orphan pool is empty"
        );
    }
}

pub struct TxPoolOrphanPartialInputUnknown;
impl Spec for TxPoolOrphanPartialInputUnknown {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let (net, (parent, tx1, tx11, tx12, tx13, final_tx)) = build_tx_chain(node0);

        assert!(
            run_replay_tx(&net, node0, parent, 0, 1),
            "parent sended expect nothing in orphan pool"
        );
        assert!(
            run_replay_tx(&net, node0, tx1, 0, 2),
            "tx1 is sent expect nothing in orphan pool"
        );
        assert!(
            run_replay_tx(&net, node0, tx11, 0, 3),
            "tx11 is sent expect nothing in orphan pool"
        );
        assert!(
            run_replay_tx(&net, node0, tx12, 0, 4),
            "tx12 is sent expect nothing in orphan pool"
        );
        assert!(
            run_replay_tx(&net, node0, final_tx, 1, 4),
            "expect final_tx is in orphan pool"
        );

        should_receive_get_relay_transactions(&net, node0, "node should ask for tx13");
        assert!(
            run_send_tx(&net, node0, tx13, 0, 6),
            "tx13 is sent, orphan pool is empty"
        );
    }
}

pub struct TxPoolOrphanDoubleSpend;
impl Spec for TxPoolOrphanDoubleSpend {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        node0.mine_until_out_bootstrap_period();
        let parent = node0.new_transaction_with_capacity(capacity_bytes!(800));

        let script = node0.always_success_script();
        let new_output1 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(200).into())
            .lock(script)
            .build();
        let new_output2 = new_output1.clone();
        let new_output3 = new_output1.clone();

        let tx1 = parent
            .as_advanced_builder()
            .set_inputs(vec![CellInput::new(OutPoint::new(parent.hash(), 0), 0)])
            .set_outputs(vec![new_output1, new_output2, new_output3])
            .set_outputs_data(vec![Default::default(); 3])
            .build();

        let tx11 =
            node0.new_transaction_with_capacity_and_index(tx1.hash(), capacity_bytes!(100), 0, 0);
        let tx12 =
            node0.new_transaction_with_capacity_and_index(tx1.hash(), capacity_bytes!(120), 0, 0);

        let mut net = Net::new(
            "orphan_tx_test",
            node0.consensus(),
            vec![SupportProtocols::RelayV3],
        );
        net.connect(node0);

        assert!(
            run_replay_tx(&net, node0, tx11, 1, 0),
            "tx11 in orphan pool"
        );

        assert!(
            run_replay_tx(&net, node0, tx12, 2, 0),
            "tx12 in orphan pool"
        );
    }
}
