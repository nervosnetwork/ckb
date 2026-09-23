use super::utils::assert_committed_at;
use crate::utils::{
    assert_send_transaction_fail, since_from_absolute_block_number, since_from_absolute_timestamp,
    since_from_relative_block_number, since_from_relative_timestamp,
};
use crate::{Node, Spec};

use ckb_types::core::BlockNumber;
use std::thread::sleep;
use std::time::Duration;

pub struct ValidSince;

// TODO add cases verify compact block(forks) including transaction of which since != 0
impl Spec for ValidSince {
    fn run(&self, nodes: &mut Vec<Node>) {
        self.test_since_relative_block_number(&nodes[0]);
        self.test_since_absolute_block_number(&nodes[0]);
        self.test_since_relative_median_time(&nodes[0]);
        self.test_since_absolute_median_time(&nodes[0]);
    }

    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.params.cellbase_maturity = Some(0);
    }
}

impl ValidSince {
    pub fn test_since_relative_block_number(&self, node: &Node) {
        node.mine_until_out_bootstrap_period();
        let started_tip_number = node.get_tip_block_number();
        let relative: BlockNumber = 10;
        let since = since_from_relative_block_number(relative);
        let transaction = {
            let cellbase = node.get_tip_block().transactions()[0].clone();
            node.new_transaction_with_since(cellbase.hash(), since)
        };

        // Failed to send transaction since SinceImmaturity
        for _ in 1..=(relative - 3) {
            assert_send_transaction_fail(
                node,
                &transaction,
                "TransactionFailedToVerify: Verification failed Transaction(Immature(",
            );
            node.mine(1);
        }

        // Success to send transaction after cellbase immaturity and since immaturity
        assert!(
            node.rpc_client()
                .send_transaction_result(transaction.data().into())
                .is_ok(),
            "transaction is ok, tip is equal to relative since block number",
        );

        assert_committed_at(node, &transaction, started_tip_number + relative);
    }

    pub fn test_since_absolute_block_number(&self, node: &Node) {
        node.mine_until_out_bootstrap_period();
        let absolute: BlockNumber = node.rpc_client().get_tip_block_number() + 10;
        let since = since_from_absolute_block_number(absolute);
        let transaction = {
            let cellbase = node.get_tip_block().transactions()[0].clone();
            node.new_transaction_with_since(cellbase.hash(), since)
        };

        // Failed to send transaction since SinceImmaturity
        let tip_number = node.rpc_client().get_tip_block_number();
        for _ in tip_number + 1..=(absolute - 3) {
            assert_send_transaction_fail(
                node,
                &transaction,
                "TransactionFailedToVerify: Verification failed Transaction(Immature(",
            );
            node.mine(1);
        }

        // Success to send transaction after cellbase immaturity and since immaturity
        assert!(
            node.rpc_client()
                .send_transaction_result(transaction.data().into())
                .is_ok(),
            "transaction is ok, tip is equal to absolute since block number",
        );

        assert_committed_at(node, &transaction, absolute);
    }

    pub fn test_since_relative_median_time(&self, node: &Node) {
        let median_time_block_count = node.consensus().median_time_block_count() as u64;
        node.mine_until_out_bootstrap_period();
        let old_median_time: u64 = node.rpc_client().get_blockchain_info().median_time.into();
        node.mine(1);
        let source_block = node.get_tip_block();
        let cellbase = source_block.transactions()[0].clone();
        sleep(Duration::from_secs(2));

        node.mine(median_time_block_count);

        // Calculate the current block median time
        let tip_number = node.rpc_client().get_tip_block_number();
        let mut timestamps: Vec<u64> = (tip_number - median_time_block_count + 1..=tip_number)
            .map(|block_number| {
                node.rpc_client()
                    .get_block_by_number(block_number)
                    .unwrap()
                    .header
                    .inner
                    .timestamp
                    .into()
            })
            .collect();
        timestamps.sort_unstable();
        let median_time = timestamps[timestamps.len() >> 1];

        // RFC 0028 starts relative timestamp since at the source block's
        // timestamp. The prior median is the pre-hardfork rule; fast mining
        // can make their difference look harmless until a boundary is tested.
        let base_time = if node
            .consensus()
            .hardfork_switch()
            .ckb2021
            .is_block_ts_as_relative_since_start_enabled(node.get_tip_block().epoch().number())
        {
            source_block.timestamp()
        } else {
            old_median_time
        };
        let median_time_seconds = (median_time - base_time) / 1000;
        {
            let since = since_from_relative_timestamp(median_time_seconds + 1);
            let transaction = node.new_transaction_with_since(cellbase.hash(), since);
            assert_send_transaction_fail(
                node,
                &transaction,
                "TransactionFailedToVerify: Verification failed Transaction(Immature(",
            );
        }
        {
            let since = since_from_relative_timestamp(median_time_seconds - 1);
            let transaction = node.new_transaction_with_since(cellbase.hash(), since);
            let result = node
                .rpc_client()
                .send_transaction_result(transaction.data().into());
            assert!(
                result.is_ok(),
                "relative since is below the canonical boundary: {result:?}"
            );
        }
    }

    pub fn test_since_absolute_median_time(&self, node: &Node) {
        let median_time_block_count = node.consensus().median_time_block_count() as u64;
        node.mine_until_out_bootstrap_period();
        let cellbase = node.get_tip_block().transactions()[0].clone();

        node.mine(median_time_block_count);

        // Calculate current block median time
        let tip_number = node.rpc_client().get_tip_block_number();
        let mut timestamps: Vec<u64> = ((tip_number - median_time_block_count + 1)..=tip_number)
            .map(|block_number| {
                node.rpc_client()
                    .get_block_by_number(block_number)
                    .unwrap()
                    .header
                    .inner
                    .timestamp
                    .into()
            })
            .collect();
        timestamps.sort_unstable();
        let median_time = timestamps[timestamps.len() >> 1];

        // Absolute since timestamp in seconds
        let median_time_seconds = median_time / 1000;
        {
            let since = since_from_absolute_timestamp(median_time_seconds + 1);
            let transaction = node.new_transaction_with_since(cellbase.hash(), since);
            assert_send_transaction_fail(
                node,
                &transaction,
                "TransactionFailedToVerify: Verification failed Transaction(Immature(",
            );
        }
        {
            let since = since_from_absolute_timestamp(median_time_seconds - 1);
            let transaction = node.new_transaction_with_since(cellbase.hash(), since);
            assert!(
                node.rpc_client()
                    .send_transaction_result(transaction.data().into())
                    .is_ok(),
                "transaction's since is greater than tip's median time",
            );
        }
    }
}
