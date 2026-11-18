use super::*;
use crate::authority::{
    notice::Class,
    tests::common::{self, accept, config, spend, store},
};

fn charge(entries: &Members) -> Amount {
    entries.values().fold(Amount::default(), |total, entry| {
        total.checked_add(owner_amount(entry).unwrap()).unwrap()
    })
}

#[test]
fn a_fresh_capacity_capture_keeps_prior_removed_owner_observations() {
    let store = store();
    let hash = accept(&store, common::tx(10620), 1, 1, Status::Pending);
    let mut plan = Plan::new(store.snapshot().0, Class::Trusted, Default::default());
    {
        let mut graph = Graph::new(&store, &mut plan);
        let previous = graph.require(&hash).unwrap();
        store.apply(common::delete(&store, previous)).unwrap();
        assert_eq!(graph.capture_accepted().unwrap(), Amount::default());
        assert!(graph.observed().is_empty());
    }
    assert!(matches!(store.apply(plan), Err(Error::Stale)));
}

#[test]
fn capacity_capture_rejects_an_insert_or_replacement_after_a_point_read() {
    let store = store();
    let transaction = common::tx(10621);
    let hash = transaction.hash();
    let mut absent = Plan::new(store.snapshot().0, Class::Trusted, Default::default());
    assert!(absent.get(&store, &hash).unwrap().is_none());
    accept(&store, transaction, 1, 1, Status::Pending);
    assert!(matches!(absent.capture_accepted(&store), Err(Error::Stale)));

    let mut present = Plan::new(store.snapshot().0, Class::Trusted, Default::default());
    let original = present.get(&store, &hash).unwrap().unwrap();
    common::replace(&store, Arc::clone(&original), original.phase.clone());
    assert!(matches!(
        present.capture_accepted(&store),
        Err(Error::Stale)
    ));
}

#[test]
fn capacity_capture_tracks_untouched_owners_by_revision() {
    let store = store();
    let hash = accept(&store, common::tx(10622), 1, 1, Status::Pending);
    let owner = store.point(&hash).1.unwrap();
    let weak_count = Arc::weak_count(&owner);
    let mut plan = Plan::new(store.snapshot().0, Class::Trusted, Default::default());
    plan.capture_accepted(&store).unwrap();
    assert_eq!(Arc::weak_count(&owner), weak_count);

    common::insert(
        &store,
        common::entry(&store, common::tx(10623), Source::Local),
    );
    store.apply(plan.clone()).unwrap();
    common::replace(&store, Arc::clone(&owner), owner.phase.clone());
    assert!(matches!(store.apply(plan), Err(Error::Stale)));
}

#[test]
fn certified_evictions_select_a_shared_family_and_keep_the_accepted_cut() {
    let pool = store();
    let root = accept(&pool, common::output_tx(10630), 0, 1, Status::Pending);
    let point = OutPoint::new(root.clone(), 0);
    let left = accept(
        &pool,
        spend(10631, &[], std::slice::from_ref(&point)),
        100,
        1,
        Status::Pending,
    );
    let right = accept(&pool, spend(10632, &[], &[point]), 100, 1, Status::Pending);
    let join = accept(
        &pool,
        spend(
            10633,
            &[],
            &[
                OutPoint::new(left.clone(), 0),
                OutPoint::new(right.clone(), 0),
            ],
        ),
        100,
        1,
        Status::Pending,
    );
    let independent = accept(&pool, common::tx(10634), 10_000, 1, Status::Pending);
    let candidate = ranked_member(&store(), 10635, 100_000, &[]);
    let admitted = AdmissionCandidate {
        entry: Arc::clone(&candidate),
        ancestors: BTreeSet::from([candidate.hash()]),
    };
    let (view, snapshot) = pool.snapshot();
    let mut plan = Plan::new(view, Class::Trusted, Default::default());
    let request = Amount {
        items: pool.budget.limits.accepted.items - 1,
        ..Amount::default()
    };
    let family = Graph::new(&pool, &mut plan)
        .certified_evictions(&admitted, request, &snapshot)
        .unwrap();
    assert_eq!(
        family,
        Some(BTreeSet::from([root.clone(), left, right, join]))
    );
    // An edit outside the certified family still participates in the premise.
    let old = pool.point(&independent).1.unwrap();
    common::replace(&pool, Arc::clone(&old), old.phase.clone());
    assert!(matches!(pool.apply(plan), Err(Error::Stale)));
    let old = pool.point(&root).1.unwrap();
    let mut accepted = old.accepted().unwrap().clone();
    accepted.forced_status = Some(Status::Proposed);
    common::replace(&pool, old, Phase::Accepted(accepted));
    let mut next = Plan::new(view, Class::Trusted, Default::default());
    let selected = Graph::new(&pool, &mut next)
        .certified_evictions(&admitted, request, &snapshot)
        .unwrap()
        .expect("pending descendants and another singleton can cover the pressure");
    let mut virtual_owners: Members = pool
        .capture_accepted()
        .owners
        .into_iter()
        .map(|owner| (owner.hash(), owner))
        .collect();
    virtual_owners.insert(candidate.hash(), Arc::clone(&candidate));
    let mut expected = BTreeSet::new();
    trim_virtual(
        &mut virtual_owners,
        &snapshot,
        &config(),
        &mut expected,
        &BTreeSet::new(),
        &admitted,
        Amount {
            items: 4,
            ..Amount::default()
        },
    )
    .unwrap();
    assert_eq!(selected, expected);
    assert_eq!(selected.len(), 4);
    assert!(!selected.contains(&root));
}

#[test]
fn certified_evictions_match_full_trimming_for_a_leaf_with_a_shared_ancestor() {
    let pool = store();
    let root = accept(&pool, common::output_tx(10640), 0, 1, Status::Pending);
    let root_output = OutPoint::new(root.clone(), 0);
    let mut parents = Vec::new();
    for nonce in [10641, 10642] {
        let hash = accept(
            &pool,
            spend(nonce, &[], std::slice::from_ref(&root_output)),
            100,
            1,
            Status::Pending,
        );
        parents.push(OutPoint::new(hash, 0));
    }
    for (nonce, fee) in [(10643, 1000), (10644, 2000)] {
        accept(&pool, common::tx(nonce), fee, 1, Status::Pending);
    }
    let raw = common::entry(&pool, spend(10645, &[], &parents), Source::Local);
    let incumbents: Members = pool
        .capture_accepted()
        .owners
        .into_iter()
        .map(|owner| (owner.hash(), owner))
        .collect();
    let (view, snapshot) = pool.snapshot();
    let mut decisions = (0, 0);
    for fee in (0..=100_000).step_by(2500) {
        let (prepared, rejection) =
            common::admission(&pool, &raw, fee, 1, Status::Pending, &config()).unwrap();
        assert!(rejection.is_none());
        let entry = Arc::clone(
            prepared
                .edits()
                .get(&raw.hash())
                .unwrap()
                .after
                .as_ref()
                .unwrap(),
        );
        let mut owners = incumbents.clone();
        owners.insert(entry.hash(), Arc::clone(&entry));
        let ancestors =
            ancestor_hashes(&owners, &entry.hash(), config().max_ancestors_count).unwrap();
        assert_eq!(ancestors.len(), 4);
        assert!(ancestors.contains(&root));
        let candidate = AdmissionCandidate { entry, ancestors };
        for count in [1, 2] {
            let excess = Amount {
                items: count,
                ..Amount::default()
            };
            let added = Amount {
                items: pool.budget.limits.accepted.items - incumbents.len() + count,
                ..Amount::default()
            };
            let mut plan = Plan::new(view, Class::Trusted, Default::default());
            let actual =
                Graph::new(&pool, &mut plan).certified_evictions(&candidate, added, &snapshot);
            let mut victims = BTreeSet::new();
            let expected = trim_virtual(
                &mut owners.clone(),
                &snapshot,
                &config(),
                &mut victims,
                &BTreeSet::new(),
                &candidate,
                excess,
            );
            match (actual, expected) {
                (Ok(Some(selected)), Ok(())) => {
                    assert_eq!(selected, victims, "fee={fee} count={count}");
                    assert!(selected.is_disjoint(&candidate.ancestors));
                    decisions.0 += 1;
                }
                (Err(Error::Rejected(Reject::Full(a))), Err(Error::Rejected(Reject::Full(b)))) => {
                    assert_eq!(a, b);
                    decisions.1 += 1;
                }
                (actual, expected) => panic!("fee={fee} count={count}: {actual:?}, {expected:?}"),
            }
        }
    }
    assert!(decisions.0 > 0 && decisions.1 > 0, "{decisions:?}");
}

fn ranked_member(store: &Store, nonce: u32, fee: u64, parents: &[Byte32]) -> Arc<Entry> {
    let deps: Vec<_> = parents
        .iter()
        .map(|hash| OutPoint::new(hash.clone(), 0))
        .collect();
    let hash = accept(store, spend(nonce, &[], &deps), fee, 1, Status::Pending);
    let owner = store.point(&hash).1.unwrap();
    let mut value = owner.accepted().unwrap().clone();
    // Normalize virtual weight to isolate rank changes from encoding length.
    value.size = 100;
    owner.with_phase(Phase::Accepted(value))
}

fn rank_fixture() -> (Arc<Store>, Members, [Byte32; 6]) {
    let store = store();
    let a = ranked_member(&store, 10600, 0, &[]);
    let b = ranked_member(&store, 10601, 1, &[a.hash()]);
    let c = ranked_member(&store, 10602, 9, &[b.hash()]);
    let d = ranked_member(&store, 10603, 100, &[a.hash()]);
    let e = ranked_member(&store, 10604, 45, &[]);
    let candidate = ranked_member(&store, 10605, 1000, &[]);
    let hashes = [
        a.hash(),
        b.hash(),
        c.hash(),
        d.hash(),
        e.hash(),
        candidate.hash(),
    ];
    let entries = [a, b, c, d, e, candidate]
        .into_iter()
        .map(|entry| (entry.hash(), entry))
        .collect();
    (store, entries, hashes)
}

fn item_limit(items: usize) -> Amount {
    Amount {
        items,
        bytes: usize::MAX,
        edges: usize::MAX,
        serialized: usize::MAX,
        cycles: u64::MAX,
    }
}

#[test]
fn trim_batch_updates_shared_ancestor_before_the_next_round() {
    let (store, original, [a, b, c, d, e, candidate]) = rank_fixture();
    let mut entries = original.clone();
    let mut removed = BTreeSet::new();
    let admission = AdmissionCandidate {
        entry: Arc::clone(entries.get(&candidate).unwrap()),
        ancestors: BTreeSet::from([candidate.clone()]),
    };
    // First remove b+c: their average rate is 5, below a's 27.5.
    // Then a's remaining average is 50, so e (45) must be selected next.
    let charge = charge(&entries);
    trim_virtual(
        &mut entries,
        &store.snapshot().1,
        &config(),
        &mut removed,
        &BTreeSet::new(),
        &admission,
        charge.positive_difference(item_limit(3)),
    )
    .unwrap();
    assert_eq!(removed, BTreeSet::from([b, c, e]));
    assert_eq!(
        entries.keys().cloned().collect::<BTreeSet<_>>(),
        BTreeSet::from([a.clone(), d, candidate])
    );
    let old_totals = aggregates(&original, config().max_ancestors_count).unwrap();
    let old_snapshot =
        entry_snapshot(original.get(&a).unwrap(), *old_totals.get(&a).unwrap()).unwrap();
    assert_eq!(old_snapshot.descendants_count, 4);
    assert_eq!(old_snapshot.descendants_fee.as_u64(), 110);
    let totals = aggregates(&entries, config().max_ancestors_count).unwrap();
    assert_eq!(totals.get(&a).unwrap().descendants.count, 2);
    assert_eq!(totals.get(&a).unwrap().descendants.fee, 100);
}

#[test]
fn trimming_continues_until_every_excess_coordinate_is_released() {
    let (store, mut entries, [_, b, c, _, e, candidate]) = rank_fixture();
    let mut removed = BTreeSet::new();
    let admission = AdmissionCandidate {
        entry: Arc::clone(entries.get(&candidate).unwrap()),
        ancestors: BTreeSet::from([candidate]),
    };
    // The first b+c closure covers the item excess, but its 200 serialized
    // bytes leave 50 more to release. The next eligible owner is e.
    trim_virtual(
        &mut entries,
        &store.snapshot().1,
        &config(),
        &mut removed,
        &BTreeSet::new(),
        &admission,
        Amount {
            items: 1,
            serialized: 250,
            ..Amount::default()
        },
    )
    .unwrap();
    assert_eq!(removed, BTreeSet::from([b, c, e]));
}

#[test]
fn trim_late_protected_rejection_keeps_the_original_notification_graph() {
    let (store, original, [a, b, c, d, e, candidate]) = rank_fixture();
    let mut entries = original.clone();
    let mut removed = BTreeSet::new();
    let admission = AdmissionCandidate {
        entry: Arc::clone(entries.get(&candidate).unwrap()),
        ancestors: BTreeSet::from([candidate.clone()]),
    };
    let before = aggregates(&original, config().max_ancestors_count).unwrap();
    let charge = charge(&entries);
    let result = trim_virtual(
        &mut entries,
        &store.snapshot().1,
        &config(),
        &mut removed,
        &BTreeSet::new(),
        &admission,
        charge.positive_difference(item_limit(0)),
    );
    assert!(matches!(result, Err(Error::Rejected(Reject::Full(_)))));
    assert_eq!(removed, BTreeSet::from([a, b, c, d, e]));
    assert_eq!(entries.keys().cloned().collect::<Vec<_>>(), vec![candidate]);
    assert_eq!(
        aggregates(&original, config().max_ancestors_count).unwrap(),
        before
    );
    assert_eq!(store.capture_accepted().owners.len(), 6);
}

#[test]
fn trim_singleton_round_still_updates_its_surviving_ancestor() {
    let store = store();
    let a = ranked_member(&store, 10610, 0, &[]);
    let b = ranked_member(&store, 10611, 1, &[a.hash()]);
    let d = ranked_member(&store, 10612, 100, &[a.hash()]);
    let e = ranked_member(&store, 10613, 45, &[]);
    let candidate = ranked_member(&store, 10614, 1000, &[]);
    let expected = BTreeSet::from([b.hash(), e.hash()]);
    let candidate_hash = candidate.hash();
    let mut entries: Members = [a, b, d, e, candidate]
        .into_iter()
        .map(|entry| (entry.hash(), entry))
        .collect();
    let mut removed = BTreeSet::new();
    let admission = AdmissionCandidate {
        entry: Arc::clone(entries.get(&candidate_hash).unwrap()),
        ancestors: BTreeSet::from([candidate_hash]),
    };
    let charge = charge(&entries);
    trim_virtual(
        &mut entries,
        &store.snapshot().1,
        &config(),
        &mut removed,
        &BTreeSet::new(),
        &admission,
        charge.positive_difference(item_limit(3)),
    )
    .unwrap();
    assert_eq!(removed, expected);
}

#[test]
fn reducing_a_skipped_root_does_not_return_it_to_eviction_order() {
    let (store, entries, [a, b, c, _, _, _]) = rank_fixture();
    let snapshot = store.snapshot().1;
    let mut ranks = EvictionRanks::new(&entries, &snapshot, config().max_ancestors_count).unwrap();
    // A transition with room for one removal skips b's two-entry family,
    // then selects c. Reducing b's remaining fee cannot make it eligible again.
    assert_eq!(ranks.pop_lowest().unwrap().hash, b);
    assert_eq!(ranks.pop_lowest().unwrap().hash, c);
    let removed = entries.get(&c).unwrap();
    let reduction = Aggregate::one(accepted(removed).unwrap());
    for ancestor in [&a, &b] {
        ranks
            .reduce(entries.get(ancestor).unwrap(), reduction)
            .unwrap();
    }
    ranks.retire(removed).unwrap();
    // b's new rate is 1; it would incorrectly precede a if reinserted.
    assert_eq!(ranks.pop_lowest().unwrap().hash, a);
    while let Some(rank) = ranks.pop_lowest() {
        assert_ne!(rank.hash, b);
        assert_ne!(rank.hash, c);
    }
}
