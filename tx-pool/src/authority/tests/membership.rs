use super::super::{
    membership::{self, AdmissionOptions},
    model::{Premises, Source, Status, hash_key},
    store::Store,
};
use super::common::*;
use crate::{callback::CallbackEvent, error::Reject};
use ckb_app_config::TxPoolConfig;
use ckb_types::{
    core::{Capacity, FeeRate, tx_pool::get_transaction_weight},
    packed::{Byte32, OutPoint},
};
use std::collections::BTreeSet;

fn hashes(store: &Store) -> BTreeSet<Byte32> {
    store
        .capture_accepted()
        .owners
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

// These fixtures have direct cell-deps, so derive their expected parents from
// transaction bodies independently of relation indexes and stored parents.
fn assert_parents_match_transactions(store: &Store) {
    let owners = store.capture_accepted().owners;
    let present: BTreeSet<_> = owners.iter().map(|owner| owner.hash()).collect();
    for owner in owners {
        let expected: BTreeSet<_> = owner
            .transaction
            .input_pts_iter()
            .chain(
                owner
                    .transaction
                    .cell_deps_iter()
                    .map(|dep| dep.out_point()),
            )
            .map(|point| point.tx_hash())
            .filter(|hash| present.contains(hash))
            .collect();
        assert_eq!(owner.accepted().unwrap().parents, expected);
    }
}

#[test]
fn late_input_and_cell_dep_readers_become_parents_on_producer_admission() {
    for dependency in [false, true] {
        let store = store();
        let producer = entry(&store, output_tx(90_000), Source::Local);
        let output = [OutPoint::new(producer.hash(), 0)];
        let reader = if dependency {
            spend(90_001, &[], &output)
        } else {
            spend(90_001, &output, &[])
        };
        let child = accept(&store, reader, 1, 1, Status::Pending);
        assert_eq!(hashes(&store), BTreeSet::from([child.clone()]));

        let (_, reject) =
            commit_admission(&store, &producer, 1, 1, Status::Pending, &config()).unwrap();
        assert!(reject.is_none());
        assert_eq!(hashes(&store), BTreeSet::from([producer.hash(), child]));
        assert_parents_match_transactions(&store);
    }
}

#[test]
fn replacement_removes_the_complete_descendant_set_with_exact_ordered_effects() {
    for dependency in [false, true] {
        let store = store();
        let input = point(201);
        let victim = accept(
            &store,
            spend(90_002, std::slice::from_ref(&input), &[]),
            1,
            1,
            Status::Pending,
        );
        let candidate = entry(&store, spend(90_003, &[input], &[]), Source::Local);
        let output = [OutPoint::new(victim.clone(), 0)];
        let reader = if dependency {
            spend(90_004, &[], &output)
        } else {
            spend(90_004, &output, &[])
        };
        let child = accept(&store, reader, 1, 1, Status::Pending);
        assert_eq!(
            hashes(&store),
            BTreeSet::from([victim.clone(), child.clone()])
        );
        assert!(store.outbox.pending_reject(&victim).is_none());
        assert!(store.outbox.pending_reject(&child).is_none());

        let (batch, reject) =
            commit_admission(&store, &candidate, 1_000_000, 1, Status::Pending, &rbf()).unwrap();
        assert!(reject.is_none());
        let callbacks: Vec<_> = batch
            .unwrap()
            .effects()
            .iter()
            .filter_map(|effect| effect.callback())
            .map(|event| match event {
                CallbackEvent::Reject(entry, _) => (entry.transaction.hash(), false),
                CallbackEvent::Pending(entry) => (entry.transaction.hash(), true),
                CallbackEvent::Proposed(_) => panic!("pending admission must stay pending"),
            })
            .collect();
        assert_eq!(
            callbacks,
            vec![(child, false), (victim, false), (candidate.hash(), true)]
        );
        assert_eq!(hashes(&store), BTreeSet::from([candidate.hash()]));
        assert_parents_match_transactions(&store);
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
    let (_, reject) =
        commit_admission(&store, &candidate, 10_000, 1, Status::Pending, &config).unwrap();
    assert!(matches!(reject, Some(Reject::RBFRejected(_))));
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
    let (_, reject) =
        commit_admission(&store, &candidate, fee - 1, 1, Status::Pending, &config).unwrap();
    assert!(matches!(reject, Some(Reject::RBFRejected(_))));
    assert_eq!(
        hashes(&store),
        BTreeSet::from([ah.clone(), bh.clone(), ch.clone()])
    );
    let (_, reject) =
        commit_admission(&store, &candidate, fee, 1, Status::Pending, &config).unwrap();
    assert!(reject.is_none());
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
fn replacement_checks_late_descendant_limits_after_removing_overlapping_victims() {
    // S -> R -> D, P/Q[/T] -> C -> X -> D. R and late C spend the same cell.
    // RBF removes R and D even though D is also reached through retained X.
    for parent_count in [2, 3] {
        let store = store();
        let configuration = TxPoolConfig {
            max_ancestors_count: 4,
            ..rbf()
        };
        let roots: Vec<_> = (90_400..90_404)
            .map(|nonce| accept(&store, output_tx(nonce), 1, 1, Status::Pending))
            .collect();
        let input = point(206);
        let parents: Vec<_> = roots[1..=parent_count]
            .iter()
            .map(|hash| OutPoint::new(hash.clone(), 0))
            .collect();
        let candidate = entry(
            &store,
            spend(90_404, std::slice::from_ref(&input), &parents),
            Source::Local,
        );
        let victim = accept(
            &store,
            spend(90_405, &[input], &[OutPoint::new(roots[0].clone(), 0)]),
            1,
            1,
            Status::Pending,
        );
        let child = accept(
            &store,
            spend(90_406, &[], &[OutPoint::new(candidate.hash(), 0)]),
            1,
            1,
            Status::Pending,
        );
        let descendant = accept(
            &store,
            spend(
                90_407,
                &[],
                &[
                    OutPoint::new(victim.clone(), 0),
                    OutPoint::new(child.clone(), 0),
                ],
            ),
            1,
            1,
            Status::Pending,
        );
        let before = store.capture_accepted().owners;
        let charged = store.accepted_usage();
        assert_parents_match_transactions(&store);
        let (_, reject) = commit_admission(
            &store,
            &candidate,
            1_000_000,
            1,
            Status::Pending,
            &configuration,
        )
        .unwrap();
        if parent_count == 2 {
            assert!(reject.is_none(), "unexpected rejection: {reject:?}");
            let expected = roots
                .into_iter()
                .chain([candidate.hash(), child.clone()])
                .collect();
            assert_eq!(hashes(&store), expected);
            assert!(store.outbox.pending_reject(&victim).is_some());
            assert!(store.outbox.pending_reject(&descendant).is_some());
            let owners = store.capture_accepted().owners;
            let totals =
                membership::aggregates(owners.iter(), configuration.max_ancestors_count).unwrap();
            assert_eq!(totals[&candidate.hash_bytes()].ancestors.count, 3);
            assert_eq!(totals[&hash_key(&child)].ancestors.count, 4);
        } else {
            assert!(matches!(
                reject,
                Some(Reject::ExceededMaximumAncestorsCount)
            ));
            assert_eq!(
                hashes(&store),
                before.iter().map(|owner| owner.hash()).collect()
            );
            assert_eq!(store.accepted_usage(), charged);
            assert!(store.outbox.pending_reject(&victim).is_none());
            assert!(store.outbox.pending_reject(&descendant).is_none());
            for owner in &before {
                assert!(std::sync::Arc::ptr_eq(
                    owner,
                    &store.point(&owner.hash()).1.unwrap()
                ));
            }
        }
        assert_parents_match_transactions(&store);
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
    let (_, reject) =
        commit_admission(&store, &candidate, 1_000_000, 1, Status::Pending, &rbf()).unwrap();
    assert!(matches!(reject, Some(Reject::RBFRejected(_))));
    assert_eq!(hashes(&store), before);
    assert!(hashes(&store).contains(&old));
}

#[test]
fn replacement_succeeds_after_conflicting_incumbent_is_removed() {
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
    let (_, reject) = commit_admission(&store, &candidate, 1, 1, Status::Pending, &rbf()).unwrap();
    assert!(reject.is_some());
    let owner = store.point(&old).1.unwrap();
    membership::commit_removal(&store, &owner, &config(), None).unwrap();
    let (_, reject) = commit_admission(&store, &candidate, 1, 1, Status::Pending, &rbf()).unwrap();
    assert!(reject.is_none());
    assert_eq!(hashes(&store), BTreeSet::from([candidate.hash()]));
}

#[test]
fn virtual_capacity_counts_a_replaced_hash_once() {
    let transaction = tx(90_020);
    let config = TxPoolConfig {
        max_tx_pool_size: transaction.data().serialized_size_in_block(),
        ..config()
    };
    let store = Store::new(crate::test_support::genesis_snapshot(), &config).unwrap();
    let hash = accept(&store, transaction.clone(), 1000, 1, Status::Pending);
    let before = store.accepted_usage();
    let candidate = entry(&store, transaction, Source::Local);
    let (_, reject) =
        commit_admission(&store, &candidate, 1000, 1, Status::Pending, &config).unwrap();
    assert!(reject.is_none());
    assert_eq!(hashes(&store), BTreeSet::from([hash]));
    assert_eq!(store.accepted_usage(), before);
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
    let (_, reject) = commit_admission(&store, &candidate, 1, 1, Status::Pending, &config).unwrap();
    assert!(matches!(reject, Some(Reject::Full(_))));
    assert_eq!(hashes(&store), BTreeSet::from([old.clone()]));
    assert_eq!(store.accepted_usage().items, 1);

    let incumbent = store.point(&old).1.unwrap();
    delete(&store, incumbent).unwrap();
    let (_, fresh) = commit_admission(&store, &candidate, 1, 1, Status::Pending, &config).unwrap();
    assert!(fresh.is_none());
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
    let (_, reject) =
        commit_admission(&store, &candidate, 1000, 1, Status::Pending, &config).unwrap();
    assert!(reject.is_none());
    assert_eq!(hashes(&store), BTreeSet::from([candidate.hash()]));
    assert_eq!(store.accepted_usage().items, 1);
}

#[test]
fn capacity_trim_preserves_candidate_ancestry_and_reports_its_late_descendants() {
    let parent = output_tx(10700);
    let transaction = spend(10701, &[], &[OutPoint::new(parent.hash(), 0)]);
    let child = spend(10702, &[], &[OutPoint::new(transaction.hash(), 0)]);
    let victim = spend(10703, &[], &[point(224)]);
    assert_eq!(
        victim.data().serialized_size_in_block(),
        transaction.data().serialized_size_in_block()
    );
    let parent_size = parent.data().serialized_size_in_block();
    let candidate_size = transaction.data().serialized_size_in_block();
    let child_size = child.data().serialized_size_in_block();
    let config = TxPoolConfig {
        max_tx_pool_size: parent_size + child_size + candidate_size,
        ..config()
    };
    let store = Store::new(crate::test_support::genesis_snapshot(), &config).unwrap();
    let parent = accept(&store, parent, 11, 3, Status::Pending);
    let child = accept(&store, child, 2000, 5, Status::Pending);
    accept(&store, victim, 1, 7, Status::Pending);
    let candidate = entry(&store, transaction, Source::Local);
    let (batch, reject) =
        commit_admission(&store, &candidate, 1000, 13, Status::Pending, &config).unwrap();
    assert!(reject.is_none());
    let batch = batch.unwrap();
    let accepted = batch
        .effects()
        .iter()
        .find_map(|effect| match effect.callback() {
            Some(CallbackEvent::Pending(value)) if value.transaction.hash() == candidate.hash() => {
                Some(value)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(accepted.ancestors_count, 2);
    assert_eq!(accepted.ancestors_size, parent_size + candidate_size);
    assert_eq!(accepted.ancestors_cycles, 16);
    assert_eq!(accepted.ancestors_fee.as_u64(), 1011);
    assert_eq!(accepted.descendants_count, 2);
    assert_eq!(accepted.descendants_size, candidate_size + child_size);
    assert_eq!(accepted.descendants_cycles, 18);
    assert_eq!(accepted.descendants_fee.as_u64(), 3000);
    assert_eq!(
        hashes(&store),
        BTreeSet::from([parent, child, candidate.hash()])
    );
    assert_parents_match_transactions(&store);
}

#[test]
fn capacity_skips_a_low_rank_family_too_large_for_one_mutation() {
    let parent = output_tx(90_100);
    let parent_point = OutPoint::new(parent.hash(), 0);
    let children: Vec<_> = (0..101)
        .map(|index| {
            spend(
                90_101 + index,
                &[point(u8::try_from(index).unwrap())],
                std::slice::from_ref(&parent_point),
            )
        })
        .collect();
    let victim = spend(90_202, &[point(201)], &[point(202)]);
    let candidate = spend(90_203, &[point(203)], &[point(204)]);
    let configuration = TxPoolConfig {
        max_tx_pool_size: parent.data().serialized_size_in_block()
            + children
                .iter()
                .map(|child| child.data().serialized_size_in_block())
                .sum::<usize>()
            + victim.data().serialized_size_in_block(),
        ..config()
    };
    let store = Store::new(crate::test_support::genesis_snapshot(), &configuration).unwrap();
    let parent_hash = accept(&store, parent.clone(), 1, 1, Status::Pending);
    let child_fee = 1_000_000;
    let child_hashes: BTreeSet<_> = children
        .iter()
        .cloned()
        .map(|child| accept(&store, child, child_fee, 1, Status::Pending))
        .collect();
    let family_weight = get_transaction_weight(parent.data().serialized_size_in_block(), 1)
        + children
            .iter()
            .map(|child| get_transaction_weight(child.data().serialized_size_in_block(), 1))
            .sum::<u64>();
    let family_rate = FeeRate::calculate(Capacity::shannons(1 + child_fee * 101), family_weight);
    let victim_weight = get_transaction_weight(victim.data().serialized_size_in_block(), 1);
    let victim_fee = family_rate.fee(victim_weight).as_u64() + victim_weight.div_ceil(1000) + 1;
    let victim_rate = FeeRate::calculate(Capacity::shannons(victim_fee), victim_weight);
    assert!(victim_rate > family_rate);
    let child_weight = get_transaction_weight(children[0].data().serialized_size_in_block(), 1);
    assert!(victim_rate < FeeRate::calculate(Capacity::shannons(child_fee), child_weight));
    let victim_hash = accept(&store, victim, victim_fee, 1, Status::Pending);

    let candidate = entry(&store, candidate, Source::Local);
    let (_, reject) = commit_admission(
        &store,
        &candidate,
        child_fee * 2,
        1,
        Status::Pending,
        &configuration,
    )
    .unwrap();
    assert!(reject.is_none());
    let accepted = hashes(&store);
    assert!(accepted.contains(&parent_hash));
    assert!(child_hashes.is_subset(&accepted));
    assert!(!accepted.contains(&victim_hash));
    assert!(accepted.contains(&candidate.hash()));
}

#[test]
fn admission_shares_one_mutation_budget_and_counts_overlapping_families_once() {
    use ckb_types::{bytes::Bytes, prelude::*};

    // One RBF victim, these late readers and a two-entry capacity family use
    // exactly 100 entries, exceed it by one, or overlap at the exact boundary.
    for (reader_count, family_is_late, fits) in
        [(97, false, true), (98, false, false), (97, true, true)]
    {
        let input = point(205);
        let victim = spend(90_300, std::slice::from_ref(&input), &[]);
        let candidate = spend(90_301, &[input], &[]);
        let candidate_output = OutPoint::new(candidate.hash(), 0);
        let capacity_parent = if family_is_late {
            spend(90_302, &[], std::slice::from_ref(&candidate_output))
        } else {
            output_tx(90_302)
        };
        let capacity_child = spend(90_303, &[], &[OutPoint::new(capacity_parent.hash(), 0)]);
        let candidate = candidate
            .as_advanced_builder()
            .witness(Bytes::from(vec![0; capacity_child.data().serialized_size_in_block()]).pack())
            .build();
        let readers: Vec<_> = (0..reader_count)
            .map(|index| spend(90_304 + index, &[], std::slice::from_ref(&candidate_output)))
            .collect();
        let configuration = TxPoolConfig {
            max_tx_pool_size: victim.data().serialized_size_in_block()
                + capacity_parent.data().serialized_size_in_block()
                + capacity_child.data().serialized_size_in_block()
                + readers
                    .iter()
                    .map(|reader| reader.data().serialized_size_in_block())
                    .sum::<usize>(),
            ..rbf()
        };
        let growth =
            candidate.data().serialized_size_in_block() - victim.data().serialized_size_in_block();
        assert!(growth > capacity_child.data().serialized_size_in_block());
        assert!(
            growth
                <= capacity_parent.data().serialized_size_in_block()
                    + capacity_child.data().serialized_size_in_block()
        );
        let store = Store::new(crate::test_support::genesis_snapshot(), &configuration).unwrap();
        accept(&store, victim, 1, 1, Status::Pending);
        accept(&store, capacity_parent, 0, 1, Status::Pending);
        accept(&store, capacity_child, 1, 1, Status::Pending);
        let readers: BTreeSet<_> = readers
            .into_iter()
            .map(|reader| accept(&store, reader, 10_000_000, 1, Status::Pending))
            .collect();
        let before = hashes(&store);
        let charged = store.accepted_usage();
        let candidate = entry(&store, candidate, Source::Local);
        let (_, reject) = commit_admission(
            &store,
            &candidate,
            1_000_000,
            1,
            Status::Pending,
            &configuration,
        )
        .unwrap();
        if fits {
            assert!(reject.is_none(), "unexpected rejection: {reject:?}");
            let mut expected = readers;
            expected.insert(candidate.hash());
            assert_eq!(hashes(&store), expected);
            assert_parents_match_transactions(&store);
        } else {
            assert!(matches!(reject, Some(Reject::Full(_))));
            assert_eq!(hashes(&store), before);
            assert_eq!(store.accepted_usage(), charged);
            assert_parents_match_transactions(&store);
        }
    }
}

#[test]
fn replacement_and_capacity_eviction_keep_distinct_rejection_reasons() {
    let input = point(202);
    let victim = spend(90_005, std::slice::from_ref(&input), &[]);
    let unrelated = spend(90_006, &[point(203)], &[]);
    let replacement = spend(90_007, &[input, point(204)], &[]);
    let config = TxPoolConfig {
        max_tx_pool_size: victim.data().serialized_size_in_block()
            + unrelated.data().serialized_size_in_block(),
        ..rbf()
    };
    assert!(
        replacement.data().serialized_size_in_block() > victim.data().serialized_size_in_block()
    );
    assert!(replacement.data().serialized_size_in_block() <= config.max_tx_pool_size);
    let store = Store::new(crate::test_support::genesis_snapshot(), &config).unwrap();
    let victim = accept(&store, victim, 1000, 1, Status::Pending);
    let unrelated = accept(&store, unrelated, 1, 1, Status::Pending);
    let candidate = entry(&store, replacement, Source::Local);

    let (batch, reject) =
        commit_admission(&store, &candidate, 1_000_000, 1, Status::Pending, &config).unwrap();
    assert!(reject.is_none());
    let batch = batch.unwrap();
    let rejections: Vec<_> = batch
        .effects()
        .iter()
        .filter_map(|effect| match effect.callback() {
            Some(CallbackEvent::Reject(entry, reason)) => Some((entry.transaction.hash(), reason)),
            _ => None,
        })
        .collect();
    assert_eq!(rejections.len(), 2);
    for (hash, reason) in rejections {
        if hash == victim {
            assert!(matches!(reason, Reject::RBFRejected(message)
                if message == &format!("replaced by tx {}", candidate.hash())));
        } else {
            assert_eq!(hash, unrelated);
            assert!(matches!(reason, Reject::Full(_)));
        }
    }
    assert_eq!(hashes(&store), BTreeSet::from([candidate.hash()]));
    for (hash, kind) in [(&victim, "RBFRejected"), (&unrelated, "Full")] {
        let encoded = store.outbox.pending_reject(hash).unwrap();
        let reject: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(reject["type"], kind);
    }
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
    let (_, reject) =
        commit_admission(&store, &candidate, 1000, 1, Status::Pending, &config).unwrap();
    assert!(matches!(
        reject,
        Some(Reject::ExceededMaximumAncestorsCount)
    ));
    assert_eq!(hashes(&store), before);
}

#[test]
fn admission_counts_the_candidate_itself_against_a_zero_ancestor_limit() {
    let store = store();
    let candidate = entry(&store, output_tx(10710), Source::Local);
    let config = TxPoolConfig {
        max_ancestors_count: 0,
        ..config()
    };
    let (_, reject) = commit_admission(&store, &candidate, 1, 1, Status::Pending, &config).unwrap();
    assert!(matches!(
        reject,
        Some(Reject::ExceededMaximumAncestorsCount)
    ));
    assert!(hashes(&store).is_empty());
}

#[test]
fn late_ancestor_limit_precedes_candidate_snapshot_fee_overflow() {
    let store = store();
    let parent = accept(&store, output_tx(10711), u64::MAX, 1, Status::Pending);
    let transaction =
        spend(10712, &[], &[OutPoint::new(parent.clone(), 0)]).fake_hash(Byte32::new([1; 32]));
    let child =
        spend(10713, &[], &[OutPoint::new(transaction.hash(), 0)]).fake_hash(Byte32::new([2; 32]));
    let child = accept(&store, child, 0, 1, Status::Pending);
    let candidate = entry(&store, transaction, Source::Local);
    let config = TxPoolConfig {
        max_ancestors_count: 2,
        ..config()
    };
    let (_, reject) = commit_admission(&store, &candidate, 1, 1, Status::Pending, &config).unwrap();
    assert!(matches!(
        reject,
        Some(Reject::ExceededMaximumAncestorsCount)
    ));
    assert_eq!(hashes(&store), BTreeSet::from([parent, child]));
    assert_parents_match_transactions(&store);
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
    membership::commit_removal(&store, &owner, &config(), None).unwrap();
    assert_eq!(hashes(&store), BTreeSet::from([spender]));
}

#[test]
fn independently_verified_admissions_sharing_only_a_read_dependency_both_commit() {
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
    let v_first = verification_fixture(&store, &first, 1, 1, Status::Pending);
    let v_second = verification_fixture(&store, &second, 1, 1, Status::Pending);
    let (_, ar) = membership::commit_admission(
        &store,
        &first,
        None,
        &v_first,
        &config(),
        AdmissionOptions::commit(),
    )
    .unwrap();
    let (_, br) = membership::commit_admission(
        &store,
        &second,
        None,
        &v_second,
        &config(),
        AdmissionOptions::commit(),
    )
    .unwrap();
    assert!(ar.is_none() && br.is_none());
    assert_eq!(hashes(&store).len(), 3);
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
    let (_, reject) =
        commit_admission(&store, &candidate, 1000, 1, Status::Pending, &config()).unwrap();
    assert!(reject.is_none());
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
    let before = store.accepted_usage();
    let candidate = entry(&store, spend(31, &[input], &[]), Source::Local);
    let reject = preview_admission(&store, &candidate, 1000, 1, Status::Pending, &rbf()).unwrap();
    assert!(reject.is_none());
    assert_eq!(hashes(&store), BTreeSet::from([old]));
    assert_eq!(store.accepted_usage(), before);
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
    membership::commit_removal(&store, &root, &config(), None).unwrap();
    assert_eq!(hashes(&store), BTreeSet::from([b]));
    assert!(
        store
            .spender(&shared, &mut Premises::default())
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
    let (_, reject) = commit_admission(&store, &candidate, 1, 1, Status::Pending, &narrow).unwrap();
    assert!(reject.is_none());
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
    let subset: Vec<_> = hashes.iter().skip(1).take(2).cloned().collect();
    let state = store.state_read();
    for selected in [hashes.as_slice(), subset.as_slice()] {
        let keys: Vec<[u8; 32]> = selected.iter().map(hash_key).collect();
        let totals = state
            .removal_totals(&keys, config().max_ancestors_count)
            .unwrap();
        for key in keys.iter() {
            let old = state.owner(key).unwrap();
            let individual = state
                .removal_totals(&[*key], config().max_ancestors_count)
                .unwrap()[key];
            assert_eq!(
                membership::entry_snapshot(old, totals[key]).unwrap(),
                membership::entry_snapshot(old, individual).unwrap(),
            );
        }
    }
}
