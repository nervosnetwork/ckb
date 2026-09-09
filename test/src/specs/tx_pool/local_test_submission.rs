use crate::{Node, Spec};
use ckb_types::packed::Byte32;

/// The integration-only submission RPC is still a Local submission: it must
/// return a definitive resolve/verify/commit result and never enter the
/// asynchronous pre-pool, whose source domain intentionally excludes Local.
pub struct LocalTestSubmissionIsDirect;

impl Spec for LocalTestSubmissionIsDirect {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        node.mine_until_out_bootstrap_period();

        let missing = node.new_transaction(Byte32::new([0x5a; 32]));
        let error = node
            .rpc_client()
            .inner()
            .send_test_transaction(missing.data().into(), None)
            .expect_err("a Local test transaction with an unknown parent is rejected");
        assert!(
            error.to_string().contains("TransactionFailedToResolve"),
            "unexpected missing-parent result: {error}"
        );

        // A typed rejection must leave the dispatcher/service healthy, and a
        // successful response must mean the transaction is already accepted.
        let valid = node.new_transaction_spend_tip_cellbase();
        let returned = node
            .rpc_client()
            .inner()
            .send_test_transaction(valid.data().into(), None)
            .expect("valid Local test transaction is committed synchronously");
        assert_eq!(returned, valid.hash().into());

        let info = node.get_tip_tx_pool_info();
        assert_eq!(info.pending.value(), 1);
        assert_eq!(info.orphan.value(), 0);
        assert_eq!(info.verify_queue_size.value(), 0);
    }
}
