use super::super::{
    membership,
    model::{Error, Source, Status},
    store::{ReadSet, Store},
};
use super::common::*;
use crate::error::Reject;
use ckb_app_config::TxPoolConfig;
use ckb_types::{
    core::FeeRate,
    packed::{Byte32, OutPoint},
};
use std::collections::BTreeSet;
fn hashes(store: &Store) -> BTreeSet<Byte32> {
    store
        .capture(true)
        .2
        .iter()
        .map(|entry| entry.hash())
        .collect()
}
fn point(byte: u8) -> OutPoint {
    OutPoint::new(Byte32::new([byte; 32]), 0)
}
fn rbf() -> TxPoolConfig {
    TxPoolConfig {
        min_rbf_rate: FeeRate::from_u64(1000),
        ..config()
    }
}

#[test]
fn low_fee_replacement_preserves_incumbent_and_returns_the_original_rejection() {
    let config = rbf();
    let store = store();
    let input = point(1);
    let old = accept(
        &store,
        spend(1, std::slice::from_ref(&input), &[]),
        10_000,
        1,
        Status::Pending,
    );
    let candidate = entry(&store, spend(2, &[input], &[]), Source::Local);
    let (plan, reject) =
        admission(&store, &candidate, 10_000, 1, Status::Pending, &config).unwrap();
    assert!(matches!(reject, Some(Reject::RBFRejected(_))));
    store.apply(plan).unwrap();
    assert_eq!(hashes(&store), BTreeSet::from([old]));
    assert!(!store.is_faulted());
}

#[test]
fn replacement_counts_shared_descendants_once_and_accepts_the_exact_fee_boundary() {
    let store = store();
    let config = rbf();
    let x = point(2);
    let y = point(3);
    let a = spend(3, std::slice::from_ref(&x), &[]);
    let b = spend(4, std::slice::from_ref(&y), &[]);
    let ah = accept(&store, a.clone(), 1000, 1, Status::Pending);
    let bh = accept(&store, b.clone(), 2000, 1, Status::Pending);
    let child = spend(
        5,
        &[OutPoint::new(a.hash(), 0), OutPoint::new(b.hash(), 0)],
        &[],
    );
    let ch = accept(&store, child, 3000, 1, Status::Pending);
    let candidate = entry(&store, spend(6, &[x, y], &[]), Source::Local);
    let fee = 6000 + candidate.transaction.data().serialized_size_in_block() as u64;
    let (_, reject) = admission(&store, &candidate, fee - 1, 1, Status::Pending, &config).unwrap();
    assert!(matches!(reject, Some(Reject::RBFRejected(_))));
    assert_eq!(
        hashes(&store),
        BTreeSet::from([ah.clone(), bh.clone(), ch.clone()])
    );
    let (plan, reject) = admission(&store, &candidate, fee, 1, Status::Pending, &config).unwrap();
    assert!(reject.is_none());
    store.apply(plan).unwrap();
    assert_eq!(hashes(&store), BTreeSet::from([candidate.hash()]));
    for hash in [ah, bh, ch] {
        assert!(
            store
                .point(&hash)
                .1
                .is_none_or(|entry| entry.accepted().is_none())
        );
    }
}

#[test]
fn replacement_cannot_add_an_unrelated_unconfirmed_input() {
    let store = store();
    let input = point(4);
    let parent = output_tx(7);
    accept(&store, parent.clone(), 1, 1, Status::Pending);
    let old = accept(
        &store,
        spend(8, std::slice::from_ref(&input), &[]),
        1,
        1,
        Status::Pending,
    );
    let before = hashes(&store);
    let candidate = entry(
        &store,
        spend(9, &[input, OutPoint::new(parent.hash(), 0)], &[]),
        Source::Local,
    );
    let (plan, reject) =
        admission(&store, &candidate, 1_000_000, 1, Status::Pending, &rbf()).unwrap();
    assert!(matches!(reject, Some(Reject::RBFRejected(_))));
    store.apply(plan).unwrap();
    assert_eq!(hashes(&store), before);
    assert!(hashes(&store).contains(&old));
}

#[test]
fn stale_replacement_rejection_rechecks_the_conflict_that_caused_it() {
    let store = store();
    let input = point(5);
    let old = accept(
        &store,
        spend(10, std::slice::from_ref(&input), &[]),
        10_000,
        1,
        Status::Pending,
    );
    let candidate = entry(&store, spend(11, &[input], &[]), Source::Local);
    let (rejected, reject) = admission(&store, &candidate, 1, 1, Status::Pending, &rbf()).unwrap();
    assert!(reject.is_some());
    let owner = store.point(&old).1.unwrap();
    store
        .apply(membership::removal(&store, &owner, &config(), None).unwrap())
        .unwrap();
    assert!(matches!(store.apply(rejected), Err(Error::Stale)));
    let (fresh, reject) = admission(&store, &candidate, 1, 1, Status::Pending, &rbf()).unwrap();
    assert!(reject.is_none());
    store.apply(fresh).unwrap();
    assert_eq!(hashes(&store), BTreeSet::from([candidate.hash()]));
}

#[test]
fn capacity_self_eviction_does_not_evict_an_incumbent() {
    let incumbent = tx(12);
    let config = TxPoolConfig {
        max_tx_pool_size: incumbent.data().serialized_size_in_block(),
        ..config()
    };
    let store = Store::new(crate::test_support::genesis_snapshot(), &config).unwrap();
    let old = accept(&store, incumbent, 1000, 1, Status::Pending);
    let candidate = entry(&store, tx(13), Source::Local);
    let (plan, reject) = admission(&store, &candidate, 1, 1, Status::Pending, &config).unwrap();
    assert!(reject.is_some());
    store.apply(plan).unwrap();
    assert_eq!(hashes(&store), BTreeSet::from([old]));
    assert_eq!(store.budget.accepted_usage().items, 1);
}

#[test]
fn capacity_replacement_is_one_atomic_candidate_and_victim_change() {
    let incumbent = tx(14);
    let config = TxPoolConfig {
        max_tx_pool_size: incumbent.data().serialized_size_in_block(),
        ..config()
    };
    let store = Store::new(crate::test_support::genesis_snapshot(), &config).unwrap();
    accept(&store, incumbent, 1, 1, Status::Pending);
    let candidate = entry(&store, tx(15), Source::Local);
    let (plan, reject) = admission(&store, &candidate, 1000, 1, Status::Pending, &config).unwrap();
    assert!(reject.is_none());
    assert_eq!(plan.edits.len(), 2);
    store.apply(plan).unwrap();
    assert_eq!(hashes(&store), BTreeSet::from([candidate.hash()]));
    assert_eq!(store.budget.accepted_usage().items, 1);
}

#[test]
fn parent_limit_rejection_keeps_existing_ancestors_unchanged() {
    let store = store();
    let root = output_tx(16);
    accept(&store, root.clone(), 1, 1, Status::Pending);
    let child = spend(17, &[OutPoint::new(root.hash(), 0)], &[]);
    accept(&store, child.clone(), 1, 1, Status::Pending);
    let before = hashes(&store);
    let candidate = entry(
        &store,
        spend(18, &[OutPoint::new(child.hash(), 0)], &[]),
        Source::Local,
    );
    let config = TxPoolConfig {
        max_ancestors_count: 2,
        ..config()
    };
    let (plan, reject) = admission(&store, &candidate, 1000, 1, Status::Pending, &config).unwrap();
    assert!(matches!(
        reject,
        Some(Reject::ExceededMaximumAncestorsCount)
    ));
    store.apply(plan).unwrap();
    assert_eq!(hashes(&store), before);
}

#[test]
fn conditional_dependency_reader_is_not_a_causal_ancestor_of_a_later_spender() {
    let store = store();
    let shared = point(6);
    let reader = accept(
        &store,
        spend(19, &[point(7)], std::slice::from_ref(&shared)),
        1,
        1,
        Status::Pending,
    );
    let spender = accept(&store, spend(20, &[shared], &[]), 1, 1, Status::Pending);
    assert!(
        store
            .point(&spender)
            .1
            .unwrap()
            .accepted()
            .unwrap()
            .parents
            .is_empty()
    );
    let owner = store.point(&reader).1.unwrap();
    let plan = membership::removal(&store, &owner, &config(), None).unwrap();
    assert_eq!(plan.edits.len(), 1);
    store.apply(plan).unwrap();
    assert_eq!(hashes(&store), BTreeSet::from([spender]));
}

#[test]
fn independently_prepared_admissions_sharing_only_a_read_dependency_both_commit() {
    let store = store();
    let parent = output_tx(21);
    accept(&store, parent.clone(), 1, 1, Status::Pending);
    let shared = OutPoint::new(parent.hash(), 0);
    let first = entry(
        &store,
        spend(22, &[point(8)], std::slice::from_ref(&shared)),
        Source::Local,
    );
    let second = entry(&store, spend(23, &[point(9)], &[shared]), Source::Local);
    let (a, ar) = admission(&store, &first, 1, 1, Status::Pending, &config()).unwrap();
    let (b, br) = admission(&store, &second, 1, 1, Status::Pending, &config()).unwrap();
    assert!(ar.is_none() && br.is_none());
    store.apply(a).unwrap();
    store.apply(b).unwrap();
    assert_eq!(hashes(&store).len(), 3);
}

#[test]
fn late_descendant_invalidates_prepared_ancestor_removal() {
    let store = store();
    let parent = output_tx(24);
    let hash = accept(&store, parent.clone(), 1, 1, Status::Pending);
    let root = store.point(&hash).1.unwrap();
    let old = membership::removal(&store, &root, &config(), None).unwrap();
    let child = accept(
        &store,
        spend(25, &[OutPoint::new(parent.hash(), 0)], &[]),
        1,
        1,
        Status::Pending,
    );
    assert!(matches!(store.apply(old), Err(Error::Stale)));
    assert_eq!(hashes(&store), BTreeSet::from([hash, child]));
}

#[test]
fn admitting_a_late_producer_updates_preexisting_child_ancestry() {
    let store = store();
    let parent = output_tx(26);
    let child = spend(27, &[OutPoint::new(parent.hash(), 0)], &[]);
    let hash = accept(&store, child, 1, 1, Status::Pending);
    assert!(
        store
            .point(&hash)
            .1
            .unwrap()
            .accepted()
            .unwrap()
            .parents
            .is_empty()
    );
    let parent = accept(&store, parent, 1, 1, Status::Pending);
    assert_eq!(
        store.point(&hash).1.unwrap().accepted().unwrap().parents,
        BTreeSet::from([parent])
    );
}

#[test]
fn a_late_producer_does_not_turn_conditional_reader_order_into_causal_ancestry() {
    let store = store();
    let input = point(10);
    let parent = spend(28, std::slice::from_ref(&input), &[]);
    let child = spend(29, &[], &[OutPoint::new(parent.hash(), 0), input]);
    let old = accept(&store, child, 1, 1, Status::Pending);
    let candidate = entry(&store, parent, Source::Local);
    let (plan, reject) =
        admission(&store, &candidate, 1000, 1, Status::Pending, &config()).unwrap();
    assert!(reject.is_none());
    store.apply(plan).unwrap();
    assert_eq!(
        hashes(&store),
        BTreeSet::from([old.clone(), candidate.hash()])
    );
    assert_eq!(
        store.point(&old).1.unwrap().accepted().unwrap().parents,
        BTreeSet::from([candidate.hash()])
    );
    assert!(
        store
            .point(&candidate.hash())
            .1
            .unwrap()
            .accepted()
            .unwrap()
            .parents
            .is_empty()
    );
}

#[test]
fn dry_run_uses_the_same_policy_without_mutating_membership_or_releasing_victim_charge() {
    let store = store();
    let input = point(11);
    let old = accept(
        &store,
        spend(30, std::slice::from_ref(&input), &[]),
        1,
        1,
        Status::Pending,
    );
    let before = store.budget.accepted_usage();
    let candidate = entry(&store, spend(31, &[input], &[]), Source::Local);
    let (mut plan, reject) =
        admission(&store, &candidate, 1000, 1, Status::Pending, &rbf()).unwrap();
    assert!(reject.is_none());
    plan.dry_run = true;
    plan.effects.clear();
    assert!(store.apply(plan).unwrap().is_none());
    assert_eq!(hashes(&store), BTreeSet::from([old]));
    assert_eq!(store.budget.accepted_usage(), before);
}

#[test]
fn accepted_removal_preserves_unrelated_shared_dependency_readers() {
    let store = store();
    let shared = point(12);
    let a = accept(
        &store,
        spend(32, &[point(13)], std::slice::from_ref(&shared)),
        1,
        1,
        Status::Pending,
    );
    let b = accept(
        &store,
        spend(33, &[point(14)], std::slice::from_ref(&shared)),
        1,
        1,
        Status::Pending,
    );
    let root = store.point(&a).1.unwrap();
    store
        .apply(membership::removal(&store, &root, &config(), None).unwrap())
        .unwrap();
    assert_eq!(hashes(&store), BTreeSet::from([b]));
    assert!(
        store
            .spender(&shared, &mut ReadSet::default())
            .unwrap()
            .is_none()
    );
}

#[test]
fn dependency_reader_fanout_does_not_consume_spender_ancestry_or_mutation_budget() {
    let store = store();
    let parent = output_tx(9000);
    let parent_hash = accept(&store, parent, 1, 1, Status::Pending);
    let shared = OutPoint::new(parent_hash.clone(), 0);
    for seed in 0..1200 {
        accept(
            &store,
            spend(9001 + seed, &[], std::slice::from_ref(&shared)),
            1,
            1,
            Status::Pending,
        );
    }
    let before = hashes(&store);
    let candidate = entry(&store, spend(10300, &[shared], &[]), Source::Local);
    let narrow = TxPoolConfig {
        max_ancestors_count: 2,
        ..config()
    };
    let (plan, reject) = admission(&store, &candidate, 1, 1, Status::Pending, &narrow).unwrap();
    assert!(reject.is_none());
    assert_eq!(plan.edits.len(), 1, "conditional readers need no mutation");
    store.apply(plan).unwrap();
    assert_eq!(
        store
            .point(&candidate.hash())
            .1
            .unwrap()
            .accepted()
            .unwrap()
            .parents,
        BTreeSet::from([parent_hash])
    );
    assert_eq!(hashes(&store).len(), before.len() + 1);
    for hash in before {
        assert!(store.point(&hash).1.unwrap().accepted().is_some());
    }
}

#[test]
fn graph_reuses_a_negative_owner_and_apply_rejects_its_successor() {
    use super::super::{notice::Class, store::Plan};
    let store = store();
    let transaction = output_tx(10400);
    let hash = transaction.hash();
    let mut graph = membership::Graph::new(&store, ReadSet::default());
    assert!(graph.get(&hash).unwrap().is_none());
    accept(&store, transaction, 1, 1, Status::Pending);
    assert!(graph.get(&hash).unwrap().is_none());
    let mut plan = Plan::new(store.snapshot().0, Class::Trusted);
    plan.reads = graph.reads;
    assert!(matches!(store.apply(plan), Err(Error::Stale)));
}

#[test]
fn graph_reuses_the_original_positive_owner_and_apply_rejects_identity_aba() {
    use super::super::{notice::Class, store::Plan};
    use std::sync::Arc;
    let store = store();
    let hash = accept(&store, output_tx(10401), 1, 1, Status::Pending);
    let old = store.point(&hash).1.unwrap();
    let mut reads = ReadSet::default();
    reads.owner(&hash, Some(&old)).unwrap();
    let current = replace(&store, Arc::clone(&old), old.phase.clone());
    assert!(!Arc::ptr_eq(&old, &current));
    let mut graph = membership::Graph::new(&store, reads);
    assert!(Arc::ptr_eq(&graph.get(&hash).unwrap().unwrap(), &old));
    let mut plan = Plan::new(store.snapshot().0, Class::Trusted);
    plan.reads = graph.reads;
    assert!(matches!(store.apply(plan), Err(Error::Stale)));
}

#[test]
fn graph_returns_stale_when_its_original_owner_has_expired() {
    use super::super::{notice::Class, store::Plan};
    use std::sync::Arc;
    let store = store();
    let old = entry(&store, output_tx(10402), Source::Local);
    let hash = old.hash();
    insert(&store, Arc::clone(&old));
    let mut reads = ReadSet::default();
    reads.owner(&hash, Some(&old)).unwrap();
    let weak = Arc::downgrade(&old);
    let mut removal = Plan::new(store.snapshot().0, Class::Trusted);
    removal.edit(Some(old), None).unwrap();
    store.apply(removal).unwrap();
    assert!(weak.upgrade().is_none());
    let mut graph = membership::Graph::new(&store, reads);
    assert!(matches!(graph.get(&hash), Err(Error::Stale)));
}

#[test]
fn graph_keeps_a_nonaccepted_observation_when_the_owner_becomes_accepted() {
    use super::super::{notice::Class, store::Plan};
    use std::sync::Arc;
    let store = store();
    let transaction = output_tx(10403);
    let old = entry(&store, transaction.clone(), Source::Local);
    let hash = old.hash();
    insert(&store, Arc::clone(&old));
    let mut graph = membership::Graph::new(&store, ReadSet::default());
    assert!(graph.get(&hash).unwrap().is_none());
    accept(&store, transaction, 1, 1, Status::Pending);
    assert!(graph.get(&hash).unwrap().is_none());
    let mut plan = Plan::new(store.snapshot().0, Class::Trusted);
    plan.reads = graph.reads;
    assert!(matches!(store.apply(plan), Err(Error::Stale)));
}

fn removal_totals_diamond(store: &Store) -> Vec<Byte32> {
    use ckb_types::{
        bytes::Bytes,
        packed::{CellDep, CellOutput},
        prelude::*,
    };
    let root = output_tx(10500)
        .as_advanced_builder()
        .cell_dep(
            CellDep::new_builder()
                .out_point(OutPoint::new(tx(10506).hash(), 0))
                .build(),
        )
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let a = accept(store, root, 11, 2, Status::Pending);
    let b = accept(
        store,
        spend(10501, &[OutPoint::new(a.clone(), 0)], &[]),
        13,
        3,
        Status::Pending,
    );
    let c = accept(
        store,
        spend(10502, &[OutPoint::new(a.clone(), 1)], &[]),
        17,
        5,
        Status::Pending,
    );
    let d = accept(
        store,
        spend(
            10503,
            &[OutPoint::new(b.clone(), 0), OutPoint::new(c.clone(), 0)],
            &[],
        ),
        19,
        7,
        Status::Pending,
    );
    vec![a, b, c, d]
}

#[test]
fn batched_removal_totals_match_individual_snapshots_with_shared_ancestors_and_descendants() {
    let store = store();
    let hashes = removal_totals_diamond(&store);
    // Also test a subset: its ancestors can be outside the captured descendant union.
    let subset: Vec<_> = hashes.iter().skip(1).take(2).cloned().collect();
    for selected in [hashes.as_slice(), subset.as_slice()] {
        let mut graph = membership::Graph::new(&store, ReadSet::default());
        let totals = graph
            .removal_totals(selected, config().max_ancestors_count)
            .unwrap();
        let mut reference = membership::Graph::new(&store, ReadSet::default());
        for hash in selected {
            let old = graph.require(hash).unwrap();
            let (ancestors, descendants) = totals.get(hash).unwrap();
            assert_eq!(
                membership::snapshot(&old, *ancestors, *descendants).unwrap(),
                reference
                    .entry_snapshot(hash, config().max_ancestors_count)
                    .unwrap(),
            );
        }
    }
}

#[test]
fn batched_removal_totals_validate_a_later_child_before_apply() {
    use super::super::{notice::Class, store::Plan};
    let store = store();
    let hashes = removal_totals_diamond(&store);
    let mut graph = membership::Graph::new(&store, ReadSet::default());
    graph
        .removal_totals(&hashes, config().max_ancestors_count)
        .unwrap();
    let leaf = hashes.last().unwrap().clone();
    accept(
        &store,
        spend(10504, &[OutPoint::new(leaf, 0)], &[]),
        23,
        11,
        Status::Pending,
    );
    let mut plan = Plan::new(store.snapshot().0, Class::Trusted);
    plan.reads = graph.reads;
    assert!(matches!(store.apply(plan), Err(Error::Stale)));
}

#[test]
fn batched_removal_totals_allow_a_later_shared_dependency_reader() {
    use super::super::{notice::Class, store::Plan};
    let store = store();
    let hashes = removal_totals_diamond(&store);
    let mut graph = membership::Graph::new(&store, ReadSet::default());
    graph
        .removal_totals(&hashes, config().max_ancestors_count)
        .unwrap();
    // Reading a captured owner's output would add a causal child and must
    // stale. This reader instead shares the root's external read-only cell.
    accept(
        &store,
        spend(10505, &[], &[OutPoint::new(tx(10506).hash(), 0)]),
        23,
        11,
        Status::Pending,
    );
    let mut plan = Plan::new(store.snapshot().0, Class::Trusted);
    plan.reads = graph.reads;
    store.apply(plan).unwrap();
}
