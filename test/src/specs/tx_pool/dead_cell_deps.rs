use crate::util::cell::gen_spendable;
use crate::util::check::is_transaction_committed;
use crate::util::mining::{mine, mine_until_bool};
use crate::util::transaction::always_success_transaction;
use crate::{Node, Spec};
use ckb_logger::info;
use ckb_types::{
    core::cell::CellMetaBuilder,
    core::{Capacity, DepType},
    packed::{CellDepBuilder, OutPoint},
    prelude::*,
};

/// There are 3 transactions, A, B and C:
///   - A was already committed before;
///   - B spends A;
///   - A is one of C's cell-deps.
///
/// A block, which commits C and B in order, should be valid.
///
/// The difference between case `CellBeingSpentThenCellDepInSameBlockTestSubmitBlock` is the order
/// of committed transactions. This case commits `[C, B]`.
pub struct CellBeingCellDepThenSpentInSameBlockTestSubmitBlock;

impl Spec for CellBeingCellDepThenSpentInSameBlockTestSubmitBlock {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        let initial_inputs = gen_spendable(node0, 2);
        let input_a = &initial_inputs[0];
        let input_c = &initial_inputs[1];

        // Commit transaction A
        let tx_a = {
            let tx_a = always_success_transaction(node0, input_a);
            node0.submit_transaction(&tx_a);
            mine_until_bool(node0, || is_transaction_committed(node0, &tx_a));
            tx_a
        };

        // Create transaction B which spends A
        let tx_b = {
            let input =
                CellMetaBuilder::from_cell_output(tx_a.output(0).unwrap(), Default::default())
                    .out_point(OutPoint::new(tx_a.hash(), 0))
                    .build();
            always_success_transaction(node0, &input)
        };

        // Create transaction C which depends A
        let tx_c = {
            let tx = always_success_transaction(node0, input_c);
            let cell_dep_to_tx_a = CellDepBuilder::default()
                .dep_type(DepType::Code.into())
                .out_point(OutPoint::new(tx_a.hash(), 0))
                .build();
            tx.as_advanced_builder().cell_dep(cell_dep_to_tx_a).build()
        };

        // Propose B and C, to prepare testing
        let block = node0
            .new_block_builder(None, None, None)
            .proposal(tx_b.proposal_short_id())
            .proposal(tx_c.proposal_short_id())
            .build();
        node0.submit_block(&block);
        mine(node0, node0.consensus().tx_proposal_window().closest());

        // Create block commits B and C in order
        let block = node0
            .new_block_builder(None, None, None)
            .transactions(vec![tx_c, tx_b])
            .build();

        let ret = node0
            .rpc_client()
            .submit_block("".to_owned(), block.data().into());
        assert!(
            ret.is_ok(),
            "a block commits transactions [C, B] should be valid, ret: {:?}",
            ret
        );
    }
}

/// There are 3 transactions, A, B and C:
///   - A was already committed before;
///   - B spends A;
///   - A is one of C's cell-deps.
///
/// A block, which commits B and C in order, should be invalid because that C's cell-dep A is dead
/// (as C spends A, A is dead).
///
/// The difference between case `CellBeingSpentThenCellDepInSameBlockTestSubmitBlock` is the order
/// of committed transactions. This case commits `[B, C]`.
pub struct CellBeingSpentThenCellDepInSameBlockTestSubmitBlock;

impl Spec for CellBeingSpentThenCellDepInSameBlockTestSubmitBlock {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        let initial_inputs = gen_spendable(node0, 2);
        let input_a = &initial_inputs[0];
        let input_c = &initial_inputs[1];

        // Commit transaction A
        let tx_a = {
            let tx_a = always_success_transaction(node0, input_a);
            node0.submit_transaction(&tx_a);
            mine_until_bool(node0, || is_transaction_committed(node0, &tx_a));
            tx_a
        };

        // Create transaction B which spends A
        let tx_b = {
            let input =
                CellMetaBuilder::from_cell_output(tx_a.output(0).unwrap(), Default::default())
                    .out_point(OutPoint::new(tx_a.hash(), 0))
                    .build();
            always_success_transaction(node0, &input)
        };

        // Create transaction C which depends A
        let tx_c = {
            let tx = always_success_transaction(node0, input_c);
            let cell_dep_to_tx_a = CellDepBuilder::default()
                .dep_type(DepType::Code.into())
                .out_point(OutPoint::new(tx_a.hash(), 0))
                .build();
            tx.as_advanced_builder().cell_dep(cell_dep_to_tx_a).build()
        };

        // Propose B and C, to prepare testing
        let block = node0
            .new_block_builder(None, None, None)
            .proposal(tx_b.proposal_short_id())
            .proposal(tx_c.proposal_short_id())
            .build();
        node0.submit_block(&block);
        mine(node0, node0.consensus().tx_proposal_window().closest());

        // Create block commits B and C in order
        let block = node0
            .new_block_builder(None, None, None)
            .transactions(vec![tx_b, tx_c])
            .build();

        let ret = node0
            .rpc_client()
            .submit_block("".to_owned(), block.data().into());
        assert!(
            ret.is_err(),
            "a block commits transactions [B, C] should be invalid, ret: {:?}",
            ret
        );
    }
}

pub struct CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplateMultiple;

impl Spec for CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplateMultiple {
    crate::setup!(num_nodes: 10);

    fn run(&self, nodes: &mut Vec<Node>) {
        while let Some(node) = nodes.pop() {
            info!(
                "Run CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate on Node.{}",
                nodes.len()
            );
            CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate {}.run(&mut vec![node])
        }
    }
}

/// There are 3 transactions, A, B and C:
///   - A was already committed before;
///   - B spends A;
///   - A is one of C's cell-deps.
///
/// Propose transactions B and C and enter the proposal window;
/// Submit transactions B and C;
/// Try to get block template and mine new blocks.
pub struct CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate;

impl Spec for CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        let initial_inputs = gen_spendable(node0, 2);
        let input_a = &initial_inputs[0];
        let input_c = &initial_inputs[1];

        // Commit transaction A
        let tx_a = {
            let tx_a = always_success_transaction(node0, input_a);
            node0.submit_transaction(&tx_a);
            mine_until_bool(node0, || is_transaction_committed(node0, &tx_a));
            tx_a
        };

        // Create transaction B which spends A
        let mut tx_b = {
            let input =
                CellMetaBuilder::from_cell_output(tx_a.output(0).unwrap(), Default::default())
                    .out_point(OutPoint::new(tx_a.hash(), 0))
                    .build();
            always_success_transaction(node0, &input)
        };

        // Create transaction C which depends A
        let mut tx_c = {
            let tx = always_success_transaction(node0, input_c);
            let cell_dep_to_tx_a = CellDepBuilder::default()
                .dep_type(DepType::Code.into())
                .out_point(OutPoint::new(tx_a.hash(), 0))
                .build();
            tx.as_advanced_builder().cell_dep(cell_dep_to_tx_a).build()
        };

        let b_weightier_than_c = rand::random::<u32>() % 2 == 0;
        if b_weightier_than_c {
            // make B's fee >> C's fee, which means B's tx-weight > C's tx-weight
            let minimum_outputs_capacity = tx_b
                .output(0)
                .unwrap()
                .as_builder()
                .build_exact_capacity(Capacity::zero())
                .unwrap()
                .capacity();
            let minimum_output = tx_b
                .output(0)
                .unwrap()
                .as_builder()
                .capacity(minimum_outputs_capacity)
                .build();
            tx_b = tx_b
                .as_advanced_builder()
                .set_outputs(vec![minimum_output])
                .build();
        } else {
            // make B's fee << C's fee, which means B's tx-weight < C's tx-weight
            let minimum_outputs_capacity = tx_c
                .output(0)
                .unwrap()
                .as_builder()
                .build_exact_capacity(Capacity::zero())
                .unwrap()
                .capacity();
            let minimum_output = tx_c
                .output(0)
                .unwrap()
                .as_builder()
                .capacity(minimum_outputs_capacity)
                .build();
            tx_c = tx_c
                .as_advanced_builder()
                .set_outputs(vec![minimum_output])
                .build();
        }

        // Propose B and C, to prepare testing
        let block = node0
            .new_block_builder(None, None, None)
            .proposal(tx_b.proposal_short_id())
            .proposal(tx_c.proposal_short_id())
            .build();
        node0.submit_block(&block);
        mine(node0, node0.consensus().tx_proposal_window().closest());

        // Submit B and C
        //
        // NOTE: It MUST submit C before B. If submit C after B, the proposed pool will reject C as
        // it thinks that B has already spent A; A is one of C's cell-deps; hence C is invalid. This
        // is current tx-pool implementation limitation but not consensus rule.
        node0.submit_transaction(&tx_c);
        node0.submit_transaction(&tx_b);

        // Inside `mine`, RPC `get_block_template` will be involved, that's our testing interface.
        mine(node0, node0.consensus().tx_proposal_window().farthest());
        if b_weightier_than_c {
            // B's tx-weight > C's tx-weight
            assert!(is_transaction_committed(node0, &tx_b));
        } else {
            // B's tx-weight < C's tx-weight,
            assert!(is_transaction_committed(node0, &tx_b));
            assert!(is_transaction_committed(node0, &tx_c));
        }
    }
}
