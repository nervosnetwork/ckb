use crate::{Node, Spec};
use ckb_jsonrpc_types::Status;
use ckb_logger::info;
use ckb_types::{
    core::{capacity_bytes, Capacity, TransactionView},
    packed::CellOutputBuilder,
    prelude::*,
};

pub struct DifferentTxsWithSameInputWithOutRBF;

impl Spec for DifferentTxsWithSameInputWithOutRBF {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();
        node0.new_block_with_blocking(|template| template.number.value() != 13);
        let tx_hash_0 = node0.generate_transaction();
        info!("Generate 2 txs with same input");
        let tx1 = node0.new_transaction(tx_hash_0.clone());
        let tx2_temp = node0.new_transaction(tx_hash_0);

        eprintln!(
            "tx1 hash: {:?} short_id: {:?}",
            tx1.hash(),
            tx1.proposal_short_id()
        );
        eprintln!(
            "tx2 hash: {:?} short_id: {:?}",
            tx2_temp.hash(),
            tx2_temp.proposal_short_id()
        );
        // Set tx2 fee to a higher value, tx1 capacity is 100, set tx2 capacity to 80 for +20 fee.
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(80).pack())
            .build();

        let tx2 = tx2_temp
            .as_advanced_builder()
            .set_outputs(vec![output])
            .build();

        eprintln!("tx1: {:?}", tx1);
        eprintln!("tx2: {:?}", tx2);

        node0.rpc_client().send_transaction(tx1.data().into());
        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        assert!(res.is_err(), "tx2 should be rejected");

        node0.mine_with_blocking(|template| template.proposals.len() != 2);
        node0.mine_with_blocking(|template| template.number.value() != 14);
        node0.mine_with_blocking(|template| template.transactions.len() != 2);

        let tip_block = node0.get_tip_block();
        let commit_txs_hash: Vec<_> = tip_block
            .transactions()
            .iter()
            .map(TransactionView::hash)
            .collect();

        // RBF (Replace-By-Fees) is not enabled
        assert!(commit_txs_hash.contains(&tx1.hash()));
        assert!(!commit_txs_hash.contains(&tx2.hash()));

        // when tx1 was confirmed, tx2 should be rejected
        // let ret = node0.rpc_client().get_transaction(tx2.hash());
        // assert!(
        //     matches!(ret.tx_status.status, Status::Rejected),
        //     "tx2 should be rejected"
        // );

        // verbosity = 1
        let ret = node0
            .rpc_client()
            .get_transaction_with_verbosity(tx1.hash(), 1);
        assert!(ret.transaction.is_none());
        assert!(matches!(ret.tx_status.status, Status::Committed));

        // let ret = node0
        //     .rpc_client()
        //     .get_transaction_with_verbosity(tx2.hash(), 1);
        // assert!(ret.transaction.is_none());
        // assert!(matches!(ret.tx_status.status, Status::Rejected));

        // verbosity = 2
        let ret = node0
            .rpc_client()
            .get_transaction_with_verbosity(tx1.hash(), 2);
        assert!(ret.transaction.is_some());
        assert!(matches!(ret.tx_status.status, Status::Committed));

        // let ret = node0
        //     .rpc_client()
        //     .get_transaction_with_verbosity(tx2.hash(), 2);
        // assert!(ret.transaction.is_none());
        // assert!(matches!(ret.tx_status.status, Status::Rejected));
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.enable_rbf = false;
    }
}
