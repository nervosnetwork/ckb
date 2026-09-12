use crate::{Node, Spec};
use ckb_jsonrpc_types::Status;
use ckb_types::{
    core::Capacity,
    packed::{self, CellDep, CellInput, CellOutputBuilder, OutPoint},
    prelude::*,
};

/// CKB allows a transaction to reference the same out-point both as an input
/// and as a cell dep. This spec verifies that the node accepts and commits
/// such a transaction.
pub struct SameCellAsInputAndCellDep;

impl Spec for SameCellAsInputAndCellDep {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        node0.mine_until_out_bootstrap_period();

        // tx_a spends the tip cellbase and creates output_a.
        let tx_a = node0.new_transaction_spend_tip_cellbase();
        node0.rpc_client().send_transaction(tx_a.data().into());

        // tx_b spends output_a and also references output_a as a cell dep.
        let output_a = OutPoint::new(tx_a.hash(), 0);
        let tx_b = tx_a
            .as_advanced_builder()
            .set_cell_deps(vec![])
            .cell_dep(node0.always_success_cell_dep())
            .cell_dep(CellDep::new_builder().out_point(output_a.clone()).build())
            .set_inputs(vec![CellInput::new(output_a, 0)])
            .set_outputs(vec![
                CellOutputBuilder::default()
                    .capacity(Capacity::bytes(100).unwrap())
                    .lock(node0.always_success_script())
                    .build(),
            ])
            .set_outputs_data(vec![packed::Bytes::default()])
            .build();

        // This must succeed; the node must not reject tx_b with Dead(out_point).
        node0.rpc_client().send_transaction(tx_b.data().into());

        // Mine until both txs are committed.
        node0.mine_with_blocking(|template| template.proposals.len() != 2);
        node0.mine(3);

        let tx_a_status = node0
            .rpc_client()
            .get_transaction(tx_a.hash())
            .tx_status
            .status;
        let tx_b_status = node0
            .rpc_client()
            .get_transaction(tx_b.hash())
            .tx_status
            .status;
        assert!(
            matches!(tx_a_status, Status::Committed),
            "tx_a should be committed"
        );
        assert!(
            matches!(tx_b_status, Status::Committed),
            "tx_b should be committed"
        );
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_fee_rate = ckb_types::core::FeeRate::from_u64(0);
    }
}
