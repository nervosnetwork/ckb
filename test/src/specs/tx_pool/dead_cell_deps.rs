use crate::util::cell::gen_spendable;
use crate::util::check::is_transaction_committed;
use crate::util::transaction::always_success_transaction;
use crate::{Node, Spec};
use ckb_types::{
    core::cell::CellMetaBuilder,
    core::{Capacity, DepType},
    packed::{CellDepBuilder, OutPoint},
    prelude::*,
};
use std::collections::BTreeSet;

/// All admitted readers precede the dep-cell spender even when they cannot fit
/// in one block. Later readers cannot extend the spender's prerequisite set.
pub struct DepReadersPrecedeSpenderAcrossBlocks;

impl Spec for DepReadersPrecedeSpenderAcrossBlocks {
    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.params.max_block_cycles = Some(2 * 537);
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.max_ancestors_count = 2;
        config.tx_pool.min_fee_rate = ckb_types::core::FeeRate::zero();
    }

    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let cells = gen_spendable(node, 8);
        let dependency = CellDepBuilder::default()
            .out_point(cells[0].out_point.clone())
            .build();
        let readers: Vec<_> = cells[1..7]
            .iter()
            .map(|cell| {
                always_success_transaction(node, cell)
                    .as_advanced_builder()
                    .cell_dep(dependency.clone())
                    .build()
            })
            .collect();
        let mut spender = always_success_transaction(node, &cells[0]);
        let output = spender
            .output(0)
            .unwrap()
            .as_builder()
            .build_exact_capacity(Capacity::zero())
            .unwrap();
        spender = spender
            .as_advanced_builder()
            .set_outputs(vec![output])
            .build();
        for reader in &readers {
            node.submit_transaction(reader);
        }
        node.submit_transaction(&spender);
        // These readers impose ordering, but are not causal ancestors of the
        // spender. The small ancestor limit must not evict any of them.
        assert_eq!(node.get_tip_tx_pool_info().pending.value(), 7);
        let late = always_success_transaction(node, &cells[7])
            .as_advanced_builder()
            .cell_dep(dependency)
            .build();
        let error = node
            .rpc_client()
            .send_transaction_result(late.data().into())
            .expect_err("readers arriving after the spender must be rejected");
        assert!(error.to_string().contains("TransactionFailedToResolve"));

        let proposal = node.new_block_with_blocking(|template| template.proposals.len() != 7);
        node.submit_block(&proposal);
        let gap = node.new_block(None, None, None);
        assert_eq!(
            gap.transactions().len(),
            1,
            "the gap block contains no commits"
        );
        node.submit_block(&gap);

        let mut remaining: BTreeSet<_> = readers.iter().map(|tx| tx.hash()).collect();
        let mut committed_spender = false;
        for count in [2, 2, 2, 1] {
            let block =
                node.new_block_with_blocking(|template| template.transactions.len() != count);
            for tx in block.transactions().iter().skip(1) {
                if tx.hash() == spender.hash() {
                    assert!(
                        remaining.is_empty(),
                        "the spender overtook an earlier reader"
                    );
                    assert!(!committed_spender);
                    committed_spender = true;
                } else {
                    assert!(
                        remaining.remove(&tx.hash()),
                        "unexpected or duplicate reader"
                    );
                }
            }
            node.submit_block(&block);
        }
        assert!(committed_spender && remaining.is_empty());
        for tx in readers.iter().chain(std::iter::once(&spender)) {
            assert!(is_transaction_committed(node, tx));
        }
        assert_eq!(node.get_tip_tx_pool_info().orphan.value(), 0);
    }
}

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
            node0.mine_until_bool(|| is_transaction_committed(node0, &tx_a));
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
                .dep_type(DepType::Code)
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
        node0.mine(node0.consensus().tx_proposal_window().closest());

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
            "a block commits transactions [C, B] should be valid, ret: {ret:?}"
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
            node0.mine_until_bool(|| is_transaction_committed(node0, &tx_a));
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
                .dep_type(DepType::Code)
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
        node0.mine(node0.consensus().tx_proposal_window().closest());

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
            "a block commits transactions [B, C] should be invalid, ret: {ret:?}"
        );
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

        for submit_spender_first in [false, true] {
            for spender_has_higher_fee in [false, true] {
                run_get_block_template_case(node0, submit_spender_first, spender_has_higher_fee);
            }
        }
    }
}

fn run_get_block_template_case(
    node0: &Node,
    submit_spender_first: bool,
    spender_has_higher_fee: bool,
) {
    // Earlier cases leave minimum-capacity outputs on chain. Exclude them so
    // the chosen high-fee transaction always has capacity available to pay it.
    let fee_headroom = Capacity::bytes(100).expect("the fixture fee fits capacity");
    let initial_inputs = gen_spendable(node0, 2)
        .into_iter()
        .filter(|cell| {
            let minimum: Capacity = cell
                .cell_output
                .clone()
                .as_builder()
                .build_exact_capacity(Capacity::zero())
                .expect("the fixture output has a valid occupied capacity")
                .capacity()
                .into();
            cell.capacity()
                >= minimum
                    .safe_add(fee_headroom)
                    .expect("the fixture capacity and fee fit")
        })
        .take(2)
        .collect::<Vec<_>>();
    assert_eq!(
        initial_inputs.len(),
        2,
        "each case needs two funded live inputs"
    );
    let input_a = &initial_inputs[0];
    let input_c = &initial_inputs[1];

    // Commit transaction A
    let tx_a = {
        let tx_a = always_success_transaction(node0, input_a);
        node0.submit_transaction(&tx_a);
        node0.mine_until_bool(|| is_transaction_committed(node0, &tx_a));
        tx_a
    };

    // Create transaction B which spends A
    let mut tx_b = {
        let input = CellMetaBuilder::from_cell_output(tx_a.output(0).unwrap(), Default::default())
            .out_point(OutPoint::new(tx_a.hash(), 0))
            .build();
        always_success_transaction(node0, &input)
    };

    // Create transaction C which depends A
    let mut tx_c = {
        let tx = always_success_transaction(node0, input_c);
        let cell_dep_to_tx_a = CellDepBuilder::default()
            .dep_type(DepType::Code)
            .out_point(OutPoint::new(tx_a.hash(), 0))
            .build();
        tx.as_advanced_builder().cell_dep(cell_dep_to_tx_a).build()
    };

    let high_fee = if spender_has_higher_fee {
        &mut tx_b
    } else {
        &mut tx_c
    };
    let minimum_outputs_capacity = high_fee
        .output(0)
        .unwrap()
        .as_builder()
        .build_exact_capacity(Capacity::zero())
        .unwrap()
        .capacity();
    let minimum_output = high_fee
        .output(0)
        .unwrap()
        .as_builder()
        .capacity(minimum_outputs_capacity)
        .build();
    *high_fee = high_fee
        .as_advanced_builder()
        .set_outputs(vec![minimum_output])
        .build();

    // Propose B and C, to prepare testing
    let block = node0
        .new_block_builder(None, None, None)
        .proposal(tx_b.proposal_short_id())
        .proposal(tx_c.proposal_short_id())
        .build();
    node0.submit_block(&block);
    node0.mine(node0.consensus().tx_proposal_window().closest());

    // An RPC reader cannot enter after its dependency has been spent in the
    // pool. Once the spender is removed, the reader may enter normally.
    if submit_spender_first {
        node0.submit_transaction(&tx_b);
        let error = node0
            .rpc_client()
            .send_transaction_result(tx_c.data().into())
            .expect_err("the pool-spent cell dep must be rejected");
        assert!(error.to_string().contains("TransactionFailedToResolve"));
        assert!(node0.rpc_client().remove_transaction(tx_b.hash()));
    }
    node0.submit_transaction(&tx_c);
    node0.submit_transaction(&tx_b);

    let fee_rate = |tx: &ckb_types::core::TransactionView| {
        let score = node0
            .rpc_client()
            .get_pool_tx_detail_info(tx.hash())
            .score_sortkey;
        assert!(score.weight.value() > 0);
        ckb_types::core::FeeRate::calculate(
            Capacity::shannons(score.fee.value()),
            score.weight.value(),
        )
    };
    let b_rate = fee_rate(&tx_b);
    let c_rate = fee_rate(&tx_c);
    assert_eq!(
        b_rate.cmp(&c_rate),
        if spender_has_higher_fee {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Less
        },
        "the accepted fees and verified weights must establish the intended order: spender_first={submit_spender_first}"
    );

    // Partial block-template publication is optimistic. Wait for the source
    // level produced by both completed direct admissions instead of consuming
    // the still-valid one-owner projection that may briefly precede it.
    let block = node0.new_block_with_blocking(|template| template.transactions.len() != 2);
    let hashes = block
        .transactions()
        .iter()
        .map(|transaction| transaction.hash())
        .collect::<Vec<_>>();
    let c_position = hashes
        .iter()
        .position(|hash| hash == &tx_c.hash())
        .expect("the dependency transaction is selected");
    let b_position = hashes
        .iter()
        .position(|hash| hash == &tx_b.hash())
        .expect("the spender transaction is selected");
    assert!(
        c_position < b_position,
        "the dependency reader must precede the same-block spender"
    );
    node0.submit_block(&block);

    assert!(is_transaction_committed(node0, &tx_b));
    assert!(is_transaction_committed(node0, &tx_c));
}
