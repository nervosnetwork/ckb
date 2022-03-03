use crate::util::transaction::relay_tx;
use crate::utils::wait_until;
use crate::{Net, Node, Spec};
use ckb_jsonrpc_types::Status;
use ckb_network::SupportProtocols;

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
            vec![SupportProtocols::Relay],
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
            vec![SupportProtocols::Relay],
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
        assert!(ret.is_some(), "reject should be recorded");
        let ret2 = ret.unwrap();
        assert!(ret2.transaction.is_none());
        assert!(matches!(ret2.tx_status.status, Status::Rejected));
    }
}
