use super::*;
use crate::test_support::{build_tx, build_tx_with_dep};
use ckb_types::packed::Byte32;

#[test]
fn persistence_fallback_orders_input_and_cell_dep_parents_before_children() {
    let root = Byte32::new([1; 32]);
    let input_parent = build_tx(vec![(&root, 0)], 1);
    let dependency_parent = build_tx(vec![(&Byte32::new([2; 32]), 0)], 1);
    let child = build_tx_with_dep(
        vec![(&input_parent.hash(), 0)],
        vec![(&dependency_parent.hash(), 0)],
        1,
    );
    let mut transactions = vec![
        child.clone(),
        dependency_parent.clone(),
        input_parent.clone(),
    ];

    sort_transactions(&mut transactions).expect("the bounded dependency sort succeeds");

    let child_position = transactions
        .iter()
        .position(|transaction| transaction.hash() == child.hash())
        .expect("child remains in the cohort");
    for parent in [&input_parent, &dependency_parent] {
        let parent_position = transactions
            .iter()
            .position(|transaction| transaction.hash() == parent.hash())
            .expect("parent remains in the cohort");
        assert!(parent_position < child_position);
    }
}

#[test]
fn persistence_fallback_preserves_fifo_for_independent_transactions() {
    let first = build_tx(vec![(&Byte32::new([3; 32]), 0)], 1);
    let second = build_tx(vec![(&Byte32::new([4; 32]), 0)], 1);
    let third = build_tx(vec![(&Byte32::new([5; 32]), 0)], 1);
    let expected = vec![second, third, first];
    let mut transactions = expected.clone();

    sort_transactions(&mut transactions).expect("independent ordering succeeds");

    assert_eq!(transactions, expected);
}
