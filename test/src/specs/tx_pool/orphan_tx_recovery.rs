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

/// Case: when an RBF replacement is itself replaced, the originally-displaced
/// transactions should be recovered back to the pool via the
/// `DeferredTask::RecoverTxs` → `ordered_resolve_queue` → `OrderedResolver`
/// pipeline path.
///
/// This exercises the "orphan tx recovery" scenario: a tx that was evicted
/// from the pool by RBF gets a second chance when the tx that took its place
/// is itself replaced, freeing the inputs that map back to the evicted tx
/// in the conflicts cache.
///
/// Setup:
///   1. Build two independent pending chains: chain A (A0 → A1 → A2) and
///      chain B (B0 → B1 → B2).
///   2. C1 consumes A1_out0 + B1_out0, displacing A2 and B2 via RBF.
///      A2 enters `conflicts_cache` keyed by A1_out0.
///      B2 enters `conflicts_cache` keyed by B1_out0.
///   3. D1 consumes A1_out0 + input_c, displacing C1 via RBF.
///      C1's freed inputs: {A1_out0, B1_out0}
///      D1's inputs: {A1_out0, input_c}
///      available = freed − D1's = {B1_out0}
///      B1_out0 maps to B2 in conflicts_outputs_cache → B2 recovered.
///      A1_out0 is consumed by D1, so A2 stays rejected.
///
/// Expected:
///   - A2 stays Rejected (input A1_out0 still consumed by D1).
///   - B2 transitions Pending → Rejected → Pending (recovered).
///   - C1 transitions Pending → Rejected.
///   - D1 ends up Pending.
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

        // ── Step 1: build chain A:  A0 → A1 → A2 ─────────────────────
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

        // ── Step 2: build chain B:  B0 → B1 → B2 ─────────────────────
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

        // ── Step 3: C1 displaces A2 and B2 via RBF ────────────────────
        //
        // C1 consumes A1_out0 + B1_out0 (does NOT consume A0_out0 or
        // B0_out0, which is critical to avoid overwriting A2/B2 entries
        // in conflicts_outputs_cache).
        //
        // After displacement:
        //   conflicts_outputs_cache[A1_out0] = A2
        //   conflicts_outputs_cache[B1_out0] = B2
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

        // ── Step 4: D1 replaces C1 via RBF ────────────────────────────
        //
        // D1 consumes A1_out0 + input_c — deliberately drops B1_out0 so
        // that B1_out0 ends up in `available_inputs` after C1 is removed:
        //
        //   freed = C1's inputs = {A1_out0, B1_out0}
        //   D1's  = {A1_out0, input_c}
        //   available = freed − D1's = {B1_out0}
        //
        // B1_out0 maps to B2 in conflicts_outputs_cache → B2 recovered!
        // A1_out0 is consumed by D1, so A2 stays rejected.
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

        // ── Assertions ────────────────────────────────────────────────
        //
        // Wait for the async recovery pipeline:
        //   submit_entry → process_rbf → DeferredTask::RecoverTxs
        //   → ordered_resolve_queue → OrderedResolver → verify → pending
        assert!(
            wait_until(30, || {
                let b2 = node.rpc_client().get_transaction(tx_b2.hash());
                b2.tx_status.status == Status::Pending
            }),
            "B2 should be recovered back to pending via ordered_resolve_queue"
        );

        // D1 should be pending (the final replacement).
        let d1_status = node.rpc_client().get_transaction(tx_d1.hash());
        assert_eq!(
            d1_status.tx_status.status,
            Status::Pending,
            "D1 (the final replacement) should be pending"
        );

        // C1 should be rejected (displaced by D1).
        let c1_status = node.rpc_client().get_transaction(tx_c1.hash());
        assert_eq!(
            c1_status.tx_status.status,
            Status::Rejected,
            "C1 should be rejected after being replaced by D1"
        );

        // A2 should still be rejected — its input A1_out0 is consumed by
        // D1, so it was NOT in the available_inputs set and was NOT
        // recovered.  Use wait_until to let deferred tasks settle before
        // asserting, preventing flaky failures under concurrent load.
        assert!(
            wait_until(15, || {
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
