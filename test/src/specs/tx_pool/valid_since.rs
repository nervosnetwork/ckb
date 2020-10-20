use crate::utils::{
    assert_send_transaction_fail, since_from_absolute_block_number, since_from_absolute_timestamp,
    since_from_relative_block_number, since_from_relative_timestamp,
};
use crate::{Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};

use ckb_types::core::BlockNumber;
use log::info;
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

        // TODO: Uncomment this case after proposed/pending pool tip verfiry logic changing
        // self.test_since_and_proposal(&nodes[1]);
    }

    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.params.cellbase_maturity = 0;
    }
}

impl ValidSince {
    pub fn test_since_relative_block_number(&self, node: &Node) {
        node.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);
        let relative: BlockNumber = 5;
        let since = since_from_relative_block_number(relative);
        let transaction = {
            let cellbase = node.get_tip_block().transactions()[0].clone();
            node.new_transaction_with_since(cellbase.hash(), since)
        };

        // Failed to send transaction since SinceImmaturity
        for _ in 1..relative {
            assert_send_transaction_fail(node, &transaction, "TransactionFailedToVerify: Immature");
            node.generate_block();
        }

        // Success to send transaction after cellbase immaturity and since immaturity
        assert!(
            node.rpc_client()
                .send_transaction_result(transaction.data().into())
                .is_ok(),
            "transaction is ok, tip is equal to relative since block number",
        );
    }

    pub fn test_since_absolute_block_number(&self, node: &Node) {
        node.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);
        let absolute: BlockNumber = node.rpc_client().get_tip_block_number() + 5;
        let since = since_from_absolute_block_number(absolute);
        let transaction = {
            let cellbase = node.get_tip_block().transactions()[0].clone();
            node.new_transaction_with_since(cellbase.hash(), since)
        };

        // Failed to send transaction since SinceImmaturity
        let tip_number = node.rpc_client().get_tip_block_number();
        for _ in tip_number + 1..absolute {
            assert_send_transaction_fail(node, &transaction, "TransactionFailedToVerify: Immature");
            node.generate_block();
        }

        // Success to send transaction after cellbase immaturity and since immaturity
        assert!(
            node.rpc_client()
                .send_transaction_result(transaction.data().into())
                .is_ok(),
            "transaction is ok, tip is equal to absolute since block number",
        );
    }

    pub fn test_since_relative_median_time(&self, node: &Node) {
        let median_time_block_count = node.consensus().median_time_block_count() as u64;
        node.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);
        let old_median_time: u64 = node.rpc_client().get_blockchain_info().median_time.into();
        node.generate_block();
        let cellbase = node.get_tip_block().transactions()[0].clone();
        sleep(Duration::from_secs(2));

        node.generate_blocks(median_time_block_count as usize);

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
        timestamps.sort();
        let median_time = timestamps[timestamps.len() >> 1];

        // Absolute since timestamp in seconds
        let median_time_seconds = (median_time - old_median_time) / 1000;
        {
            let since = since_from_relative_timestamp(median_time_seconds + 1);
            let transaction = node.new_transaction_with_since(cellbase.hash(), since);
            assert_send_transaction_fail(node, &transaction, "TransactionFailedToVerify: Immature");
        }
        {
            let since = since_from_relative_timestamp(median_time_seconds - 1);
            let transaction = node.new_transaction_with_since(cellbase.hash(), since);
            assert!(
                node.rpc_client()
                    .send_transaction_result(transaction.data().into())
                    .is_ok(),
                "transaction's since is greater than tip's median time",
            );
        }
    }

    pub fn test_since_absolute_median_time(&self, node: &Node) {
        let median_time_block_count = node.consensus().median_time_block_count() as u64;
        node.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);
        let cellbase = node.get_tip_block().transactions()[0].clone();

        node.generate_blocks(median_time_block_count as usize);

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
        timestamps.sort();
        let median_time = timestamps[timestamps.len() >> 1];

        // Absolute since timestamp in seconds
        let median_time_seconds = median_time / 1000;
        {
            let since = since_from_absolute_timestamp(median_time_seconds + 1);
            let transaction = node.new_transaction_with_since(cellbase.hash(), since);
            assert_send_transaction_fail(node, &transaction, "TransactionFailedToVerify: Immature");
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

    #[allow(clippy::identity_op)]
    pub fn test_since_and_proposal(&self, node: &Node) {
        node.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);

        // test relative block number since
        info!("Use tip block cellbase as tx input with a relative block number since");
        let relative_blocks: BlockNumber = 5;
        let since = (0b1000_0000 << 56) + relative_blocks;
        let tip_block = node.get_tip_block();
        let tx = node.new_transaction_with_since(tip_block.transactions()[0].hash(), since);

        (0..relative_blocks - DEFAULT_TX_PROPOSAL_WINDOW.0).for_each(|i| {
            info!("Tx is Immature in block N + {}", i);
            assert_send_transaction_fail(node, &tx, "TransactionFailedToVerify: Immature");
            node.generate_block();
        });

        info!(
            "Tx will be added to pending pool in N + {} block",
            relative_blocks - DEFAULT_TX_PROPOSAL_WINDOW.0
        );
        let tx_hash = node.rpc_client().send_transaction(tx.data().into());
        assert_eq!(tx_hash, tx.hash());
        node.assert_tx_pool_size(1, 0);

        info!(
            "Tx will be added to proposed pool in N + {} block",
            relative_blocks
        );
        (0..DEFAULT_TX_PROPOSAL_WINDOW.0).for_each(|_| {
            node.generate_block();
        });
        node.assert_tx_pool_size(0, 1);

        node.generate_block();
        node.assert_tx_pool_size(0, 0);

        // test absolute block number since
        let tip_number: BlockNumber = node.rpc_client().get_tip_block_number();
        info!(
            "Use tip block {} cellbase as tx input with an absolute block number since",
            tip_number
        );
        let absolute_block: BlockNumber = 10;
        let since = (0b0000_0000 << 56) + absolute_block;
        let tip_block = node.get_tip_block();
        let tx = node.new_transaction_with_since(tip_block.transactions()[0].hash(), since);

        (tip_number..absolute_block - DEFAULT_TX_PROPOSAL_WINDOW.0).for_each(|i| {
            info!("Tx is Immature in block {}", i);
            assert_send_transaction_fail(node, &tx, "Not mature cause of since condition");
            node.generate_block();
        });

        info!(
            "Tx will be added to pending pool in {} block",
            absolute_block - DEFAULT_TX_PROPOSAL_WINDOW.0
        );
        let tx_hash = node.rpc_client().send_transaction(tx.data().into());
        assert_eq!(tx_hash, tx.hash());
        node.assert_tx_pool_size(1, 0);

        info!(
            "Tx will be added to proposed pool in {} block",
            absolute_block
        );
        (0..DEFAULT_TX_PROPOSAL_WINDOW.0).for_each(|_| {
            node.generate_block();
        });
        node.assert_tx_pool_size(0, 1);

        node.generate_block();
        node.assert_tx_pool_size(0, 0);
    }
}
