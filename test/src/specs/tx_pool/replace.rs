use crate::{Node, Spec};
use ckb_jsonrpc_types::Status;
use ckb_logger::info;
use ckb_types::{
    core::{capacity_bytes, Capacity, TransactionView},
    packed::{Byte32, CellInput, CellOutputBuilder, OutPoint},
    prelude::*,
};

pub struct RbfBasic;
impl Spec for RbfBasic {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();
        node0.new_block_with_blocking(|template| template.number.value() != 13);
        let tx_hash_0 = node0.generate_transaction();
        info!("Generate 2 txs with same input");
        let tx1 = node0.new_transaction(tx_hash_0.clone());
        let tx2_temp = node0.new_transaction(tx_hash_0);

        // Set tx2 fee to a higher value, tx1 capacity is 100, set tx2 capacity to 80 for +20 fee.
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(80).pack())
            .build();

        let tx2 = tx2_temp
            .as_advanced_builder()
            .set_outputs(vec![output])
            .build();

        node0.rpc_client().send_transaction(tx1.data().into());
        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        assert!(res.is_ok(), "tx2 should replace old tx");

        node0.mine_with_blocking(|template| template.proposals.len() != 2);
        node0.mine_with_blocking(|template| template.number.value() != 14);
        node0.mine_with_blocking(|template| template.transactions.len() != 2);

        let tip_block = node0.get_tip_block();
        let commit_txs_hash: Vec<_> = tip_block
            .transactions()
            .iter()
            .map(TransactionView::hash)
            .collect();

        // RBF (Replace-By-Fees) is enabled
        assert!(!commit_txs_hash.contains(&tx1.hash()));
        assert!(commit_txs_hash.contains(&tx2.hash()));

        // when tx1 was confirmed, tx2 should be rejected
        let ret = node0.rpc_client().get_transaction(tx2.hash());
        assert!(
            matches!(ret.tx_status.status, Status::Committed),
            "tx2 should be committed"
        );

        // verbosity = 1
        let ret = node0
            .rpc_client()
            .get_transaction_with_verbosity(tx1.hash(), 1);
        assert!(ret.transaction.is_none());
        assert!(matches!(ret.tx_status.status, Status::Rejected));
        assert!(ret.tx_status.reason.unwrap().contains("RBFRejected"));

        // verbosity = 2
        let ret = node0
            .rpc_client()
            .get_transaction_with_verbosity(tx2.hash(), 2);
        assert!(ret.transaction.is_some());
        assert!(matches!(ret.tx_status.status, Status::Committed));

        let ret = node0
            .rpc_client()
            .get_transaction_with_verbosity(tx1.hash(), 2);
        assert!(ret.transaction.is_none());
        assert!(matches!(ret.tx_status.status, Status::Rejected));
        assert!(ret.tx_status.reason.unwrap().contains("RBFRejected"));
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}

pub struct RbfSameInput;
impl Spec for RbfSameInput {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();
        node0.new_block_with_blocking(|template| template.number.value() != 13);
        let tx_hash_0 = node0.generate_transaction();
        info!("Generate 2 txs with same input");
        let tx1 = node0.new_transaction(tx_hash_0.clone());
        let tx2_temp = node0.new_transaction(tx_hash_0);

        let tx2 = tx2_temp.as_advanced_builder().build();

        node0.rpc_client().send_transaction(tx1.data().into());
        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        assert!(res.is_err(), "tx2 should be rejected");
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}

pub struct RbfOnlyForResolveDead;
impl Spec for RbfOnlyForResolveDead {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();
        node0.new_block_with_blocking(|template| template.number.value() != 13);

        let tx_hash_0 = node0.generate_transaction();

        let tx1 = node0.new_transaction(tx_hash_0.clone());
        let tx1_clone = tx1.clone();

        // This is an unknown input
        let tx_hash_1 = Byte32::zero();
        let tx2 = tx1_clone
            .as_advanced_builder()
            .set_inputs(vec![{
                CellInput::new_builder()
                    .previous_output(OutPoint::new(tx_hash_1, 0))
                    .build()
            }])
            .build();

        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        let message = res.err().unwrap().to_string();
        assert!(message.contains("TransactionFailedToResolve: Resolve failed Unknown"));
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}

pub struct RbfSameInputwithLessFee;

// RBF Rule #3, #4
impl Spec for RbfSameInputwithLessFee {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();
        node0.new_block_with_blocking(|template| template.number.value() != 13);
        let tx_hash_0 = node0.generate_transaction();
        info!("Generate 2 txs with same input");
        let tx1 = node0.new_transaction(tx_hash_0.clone());
        let tx2_temp = node0.new_transaction(tx_hash_0);

        let output1 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(80).pack())
            .build();

        let tx1 = tx1.as_advanced_builder().set_outputs(vec![output1]).build();

        // Set tx2 fee to a lower value
        let output2 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(90).pack())
            .build();

        let tx2 = tx2_temp
            .as_advanced_builder()
            .set_outputs(vec![output2])
            .build();

        node0.rpc_client().send_transaction(tx1.data().into());
        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        assert!(res.is_err(), "tx2 should be rejected");
        let message = res.err().unwrap().to_string();
        assert!(message.contains(
            "Tx's current fee is 1000000000, expect it to >= 2000000363 to replace old txs"
        ));
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}

pub struct RbfTooManyDescendants;

// RBF Rule #5
impl Spec for RbfTooManyDescendants {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();

        // build txs chain
        let tx0 = node0.new_transaction_spend_tip_cellbase();
        let tx0_temp = tx0.clone();
        let mut txs = vec![tx0];
        let max_count = 101;
        while txs.len() <= max_count {
            let parent = txs.last().unwrap();
            let child = parent
                .as_advanced_builder()
                .set_inputs(vec![{
                    CellInput::new_builder()
                        .previous_output(OutPoint::new(parent.hash(), 0))
                        .build()
                }])
                .set_outputs(vec![parent.output(0).unwrap()])
                .build();
            txs.push(child);
        }
        assert_eq!(txs.len(), max_count + 1);
        // send tx chain
        for tx in txs[..=max_count - 1].iter() {
            let ret = node0.rpc_client().send_transaction_result(tx.data().into());
            assert!(ret.is_ok());
        }

        // Set tx2 fee to a higher value
        let output2 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(70).pack())
            .build();

        let tx2 = tx0_temp
            .as_advanced_builder()
            .set_outputs(vec![output2])
            .build();

        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        assert!(res.is_err(), "tx2 should be rejected");
        assert!(res
            .err()
            .unwrap()
            .to_string()
            .contains("Tx conflict too many txs"));
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}

pub struct RbfContainNewTx;

// RBF Rule #2
impl Spec for RbfContainNewTx {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();

        // build txs chain
        let tx0 = node0.new_transaction_spend_tip_cellbase();
        let mut txs = vec![tx0];
        let max_count = 5;
        while txs.len() <= max_count {
            let parent = txs.last().unwrap();
            let child = parent
                .as_advanced_builder()
                .set_inputs(vec![{
                    CellInput::new_builder()
                        .previous_output(OutPoint::new(parent.hash(), 0))
                        .build()
                }])
                .set_outputs(vec![parent.output(0).unwrap()])
                .build();
            txs.push(child);
        }
        assert_eq!(txs.len(), max_count + 1);
        // send tx chain
        for tx in txs[..=max_count - 1].iter() {
            let ret = node0.rpc_client().send_transaction_result(tx.data().into());
            assert!(ret.is_ok());
        }

        let clone_tx = txs[2].clone();
        // Set tx2 fee to a higher value
        let output2 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(70).pack())
            .build();

        let tx2 = clone_tx
            .as_advanced_builder()
            .set_inputs(vec![
                {
                    CellInput::new_builder()
                        .previous_output(OutPoint::new(txs[1].hash(), 0))
                        .build()
                },
                {
                    CellInput::new_builder()
                        .previous_output(OutPoint::new(txs[4].hash(), 0))
                        .build()
                },
            ])
            .set_outputs(vec![output2])
            .build();

        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        assert!(res.is_err(), "tx2 should be rejected");
        assert!(res
            .err()
            .unwrap()
            .to_string()
            .contains("new Tx contains unconfirmed inputs"));
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}

pub struct RbfContainInvalidInput;

// RBF Rule #2
impl Spec for RbfContainInvalidInput {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();

        // build txs chain
        let tx0 = node0.new_transaction_spend_tip_cellbase();
        let mut txs = vec![tx0];
        let max_count = 5;
        while txs.len() <= max_count {
            let parent = txs.last().unwrap();
            let child = parent
                .as_advanced_builder()
                .set_inputs(vec![{
                    CellInput::new_builder()
                        .previous_output(OutPoint::new(parent.hash(), 0))
                        .build()
                }])
                .set_outputs(vec![parent.output(0).unwrap()])
                .build();
            txs.push(child);
        }
        assert_eq!(txs.len(), max_count + 1);
        // send Tx chain
        for tx in txs[..=max_count - 1].iter() {
            let ret = node0.rpc_client().send_transaction_result(tx.data().into());
            assert!(ret.is_ok());
        }

        let clone_tx = txs[2].clone();
        // Set tx2 fee to a higher value
        let output2 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(70).pack())
            .build();

        let tx2 = clone_tx
            .as_advanced_builder()
            .set_inputs(vec![
                {
                    CellInput::new_builder()
                        .previous_output(OutPoint::new(txs[1].hash(), 0))
                        .build()
                },
                {
                    CellInput::new_builder()
                        .previous_output(OutPoint::new(txs[3].hash(), 0))
                        .build()
                },
            ])
            .set_outputs(vec![output2])
            .build();

        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        assert!(res.is_err(), "tx2 should be rejected");
        assert!(res
            .err()
            .unwrap()
            .to_string()
            .contains("new Tx contains inputs in descendants of to be replaced Tx"));
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}

pub struct RbfRejectReplaceProposed;

// RBF Rule #6
impl Spec for RbfRejectReplaceProposed {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();

        // build txs chain
        let tx0 = node0.new_transaction_spend_tip_cellbase();
        let mut txs = vec![tx0];
        let max_count = 5;
        while txs.len() <= max_count {
            let parent = txs.last().unwrap();
            let child = parent
                .as_advanced_builder()
                .set_inputs(vec![{
                    CellInput::new_builder()
                        .previous_output(OutPoint::new(parent.hash(), 0))
                        .build()
                }])
                .set_outputs(vec![parent.output(0).unwrap()])
                .build();
            txs.push(child);
        }
        assert_eq!(txs.len(), max_count + 1);
        // send Tx chain
        for tx in txs[..=max_count - 1].iter() {
            let ret = node0.rpc_client().send_transaction_result(tx.data().into());
            assert!(ret.is_ok());
        }

        node0.mine_with_blocking(|template| template.proposals.len() != max_count);
        let ret = node0.rpc_client().get_transaction(txs[2].hash());
        assert!(
            matches!(ret.tx_status.status, Status::Pending),
            "tx1 should be pending"
        );
        node0.mine(1);
        let ret = node0.rpc_client().get_transaction(txs[2].hash());
        assert!(
            matches!(ret.tx_status.status, Status::Proposed),
            "tx1 should be proposed"
        );

        let clone_tx = txs[2].clone();
        // Set tx2 fee to a higher value
        let output2 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(70).pack())
            .build();

        let tx2 = clone_tx
            .as_advanced_builder()
            .set_outputs(vec![output2])
            .build();

        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        assert!(res.is_err(), "tx2 should be rejected");
        assert!(res
            .err()
            .unwrap()
            .to_string()
            .contains("all conflict Txs should be in Pending status"));
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}
