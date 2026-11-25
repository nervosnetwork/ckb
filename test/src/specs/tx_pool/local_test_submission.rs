use crate::{Node, Spec};
use ckb_types::packed::Byte32;

/// Test submission resolves locally, then acknowledges queued verification.
pub struct LocalTestSubmissionQueuesVerification;

impl Spec for LocalTestSubmissionQueuesVerification {
    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.max_tx_verify_workers = 0;
    }

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

        // No background verifier can accept this transaction before the query.
        // The RPC still completes after resolving and installing queued work.
        let valid = node.new_transaction_spend_tip_cellbase();
        let returned = node
            .rpc_client()
            .inner()
            .send_test_transaction(valid.data().into(), None)
            .expect("resolved Local test transaction is queued");
        assert_eq!(returned, valid.hash().into());

        let info = node.get_tip_tx_pool_info();
        assert_eq!(info.pending.value(), 0);
        assert_eq!(info.orphan.value(), 0);
        assert_eq!(info.verify_queue_size.value(), 1);

        // Ordinary RPC submission still verifies and accepts the queued owner
        // synchronously, even with no background worker to consume the queue.
        node.rpc_client().send_transaction(valid.data().into());
        let info = node.get_tip_tx_pool_info();
        assert_eq!(info.pending.value(), 1);
        assert_eq!(info.verify_queue_size.value(), 0);
    }
}
