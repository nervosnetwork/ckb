use crate::{
    Node, Spec,
    util::{cell::gen_spendable, transaction::always_success_transaction},
    utils::wait_until,
};
use ckb_jsonrpc_types::Status;
use ckb_types::{
    core::{Capacity, capacity_bytes, cell::CellMetaBuilder},
    packed::{CellInput, CellOutputBuilder, OutPoint},
    prelude::*,
};

/// Replacing an RBF winner can free a displaced transaction's inputs and allow
/// its retained Replaced owner to return through resolution and verification.
///
/// Two pending chains, A0 → A1 → A2 and B0 → B1 → B2, share no inputs.
/// C1 spends A1_out0 and B1_out0, displacing A2 and B2. D1 then replaces C1
/// while spending A1_out0 and an independent confirmed input. B1_out0 becomes
/// available, satisfying B2's recovery condition; A1_out0 stays spent by D1.
/// The final pool must accept D1 and recovered B2, with C1 and A2 rejected.
pub struct RbfOrphanRecovery;

impl Spec for RbfOrphanRecovery {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];

        node.mine_until_out_bootstrap_period();
        node.new_block_with_blocking(|t| t.number.value() != 13);

        // Three independent, spendable cells from cellbase.
        let initial_inputs = gen_spendable(node, 3);
        let input_a = &initial_inputs[0];
        let input_b = &initial_inputs[1];
        let input_c = &initial_inputs[2];

        let input_c_cell = CellInput::new_builder()
            .previous_output(input_c.out_point.clone())
            .build();

        // Build chain A: A0 → A1 → A2.
        let tx_a0 = always_success_transaction(node, input_a);
        node.submit_transaction(&tx_a0);

        let a1_meta =
            CellMetaBuilder::from_cell_output(tx_a0.output(0).unwrap(), Default::default())
                .out_point(OutPoint::new(tx_a0.hash(), 0))
                .build();
        let tx_a1 = always_success_transaction(node, &a1_meta);
        let _ = node.rpc_client().send_transaction(tx_a1.data().into());

        let a2_meta =
            CellMetaBuilder::from_cell_output(tx_a1.output(0).unwrap(), Default::default())
                .out_point(OutPoint::new(tx_a1.hash(), 0))
                .build();
        let tx_a2 = always_success_transaction(node, &a2_meta);
        let _ = node.rpc_client().send_transaction(tx_a2.data().into());

        // Build chain B: B0 → B1 → B2.
        let tx_b0 = always_success_transaction(node, input_b);
        node.submit_transaction(&tx_b0);

        let b1_meta =
            CellMetaBuilder::from_cell_output(tx_b0.output(0).unwrap(), Default::default())
                .out_point(OutPoint::new(tx_b0.hash(), 0))
                .build();
        let tx_b1 = always_success_transaction(node, &b1_meta);
        let _ = node.rpc_client().send_transaction(tx_b1.data().into());

        let b2_meta =
            CellMetaBuilder::from_cell_output(tx_b1.output(0).unwrap(), Default::default())
                .out_point(OutPoint::new(tx_b1.hash(), 0))
                .build();
        let tx_b2 = always_success_transaction(node, &b2_meta);
        let _ = node.rpc_client().send_transaction(tx_b2.data().into());

        // Wait for all chain txs to reach pending.
        assert!(
            wait_until(15, || {
                let a2 = node.rpc_client().get_transaction(tx_a2.hash());
                let b2 = node.rpc_client().get_transaction(tx_b2.hash());
                a2.tx_status.status == Status::Pending && b2.tx_status.status == Status::Pending
            }),
            "chain txs should reach pending"
        );

        // C1 displaces A2 and B2 while preserving their accepted ancestors.
        // Their retained history is blocked on A1_out0 and B1_out0 respectively.
        let input_a1 = CellInput::new_builder()
            .previous_output(OutPoint::new(tx_a1.hash(), 0))
            .build();
        let input_b1 = CellInput::new_builder()
            .previous_output(OutPoint::new(tx_b1.hash(), 0))
            .build();
        let tx_c1 = tx_a2
            .as_advanced_builder()
            .set_inputs(vec![input_a1, input_b1])
            .set_outputs(vec![
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(200))
                    .build(),
            ])
            .build();
        let _ = node.rpc_client().send_transaction(tx_c1.data().into());

        // A2 and B2 should be rejected (displaced by C1).
        assert!(
            wait_until(15, || {
                let c1 = node.rpc_client().get_transaction(tx_c1.hash());
                let a2 = node.rpc_client().get_transaction(tx_a2.hash());
                let b2 = node.rpc_client().get_transaction(tx_b2.hash());
                c1.tx_status.status == Status::Pending
                    && a2.tx_status.status == Status::Rejected
                    && b2.tx_status.status == Status::Rejected
            }),
            "C1 should be pending, A2 and B2 should be rejected"
        );

        // D1 replaces C1 and releases B1_out0, allowing B2 to recover.
        // It still spends A1_out0, so A2's recovery condition remains blocked.
        let tx_d1 = tx_c1
            .as_advanced_builder()
            .set_inputs(vec![
                CellInput::new_builder()
                    .previous_output(OutPoint::new(tx_a1.hash(), 0))
                    .build(),
                input_c_cell,
            ])
            .set_outputs(vec![
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(100))
                    .build(),
            ])
            .build();
        let _ = node.rpc_client().send_transaction(tx_d1.data().into());

        // B2 must finish recovery while A2 stays blocked. Observe the complete
        // replacement and recovery outcome together, including both victims.
        assert!(
            wait_until(30, || {
                let d1 = node.rpc_client().get_transaction(tx_d1.hash());
                let b2 = node.rpc_client().get_transaction(tx_b2.hash());
                let c1 = node.rpc_client().get_transaction(tx_c1.hash());
                let a2 = node.rpc_client().get_transaction(tx_a2.hash());
                d1.tx_status.status == Status::Pending
                    && b2.tx_status.status == Status::Pending
                    && c1.tx_status.status == Status::Rejected
                    && a2.tx_status.status == Status::Rejected
            }),
            "Stable state: D1 Pending, B2 Pending (recovered), C1 Rejected, A2 Rejected"
        );
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}
