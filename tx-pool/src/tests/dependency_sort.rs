use super::*;
use crate::test_support::{build_tx, build_tx_with_dep};
use ckb_types::{bytes::Bytes, packed::Byte32};

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

#[test]
fn dependency_sort_checks_output_range_for_inputs_and_cell_deps() {
    let parent = build_tx(vec![(&Byte32::new([21; 32]), 0)], 3);
    let valid = build_tx_with_dep(
        vec![(&parent.hash(), 2)],
        vec![(&parent.hash(), 0), (&parent.hash(), 2)],
        1,
    );
    let absent = build_tx_with_dep(
        vec![(&parent.hash(), 3)],
        vec![(&parent.hash(), u32::MAX)],
        1,
    );
    let mut transactions = vec![absent.clone(), valid.clone(), parent.clone()];
    sort_transactions(&mut transactions).unwrap();
    assert_eq!(transactions, vec![absent, parent, valid]);

    let no_outputs = build_tx(vec![(&Byte32::new([22; 32]), 0)], 0);
    let absent = build_tx(vec![(&no_outputs.hash(), 0)], 1);
    let expected = vec![absent, no_outputs];
    let mut transactions = expected.clone();
    sort_transactions(&mut transactions).unwrap();
    assert_eq!(transactions, expected);
}

#[test]
fn dependency_sort_uses_the_last_witness_variant_as_producer() {
    let parent = build_tx(vec![(&Byte32::new([23; 32]), 0)], 1);
    let first = parent
        .as_advanced_builder()
        .witness(Bytes::from_static(b"first").pack())
        .build();
    let last = parent
        .as_advanced_builder()
        .witness(Bytes::from_static(b"last").pack())
        .build();
    assert_eq!(first.hash(), last.hash());
    assert_ne!(first.witness_hash(), last.witness_hash());
    let child = build_tx(vec![(&parent.hash(), 0)], 1);
    let middle = build_tx(vec![(&Byte32::new([24; 32]), 0)], 1);
    let other = build_tx(vec![(&middle.hash(), 0)], 1);
    let mut transactions = vec![
        child.clone(),
        first.clone(),
        other.clone(),
        middle.clone(),
        last.clone(),
    ];
    sort_transactions(&mut transactions).unwrap();
    // The intervening ready parent makes first-vs-last producer selection
    // observable in the order of the two newly unblocked children.
    assert_eq!(transactions, vec![first, middle, last, other, child]);
}

#[test]
fn dependency_sort_keeps_cycle_order_and_ignores_self_edges() {
    let a_hash = Byte32::new([25; 32]);
    let b_hash = Byte32::new([26; 32]);
    let a = build_tx(vec![(&b_hash, 0)], 1).fake_hash(a_hash.clone());
    let b = build_tx(vec![(&a_hash, 0)], 1).fake_hash(b_hash);
    let independent = build_tx(vec![(&Byte32::new([27; 32]), 0)], 1);
    let expected = vec![a, independent.clone(), b];
    let mut transactions = expected.clone();
    sort_transactions(&mut transactions).unwrap();
    assert_eq!(transactions, expected);

    let self_hash = Byte32::new([28; 32]);
    let self_reference = build_tx(vec![(&self_hash, 0)], 1).fake_hash(self_hash);
    let expected = vec![self_reference, independent];
    let mut transactions = expected.clone();
    sort_transactions(&mut transactions).unwrap();
    assert_eq!(transactions, expected);
}
