use super::*;
use crate::authority::tests::common::{accept, config, spend, store};

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
    let mut causes = BTreeMap::new();
    // First remove b+c: their average rate is 5, below a's 27.5.
    // Then a's remaining average is 50, so e (45) must be selected next.
    trim_virtual(
        &mut entries,
        &store.snapshot().1,
        &config(),
        &mut removed,
        &mut causes,
        &BTreeSet::new(),
        &candidate,
        item_limit(3),
    )
    .unwrap();
    assert_eq!(removed, BTreeSet::from([b, c, e]));
    assert_eq!(
        entries.keys().cloned().collect::<BTreeSet<_>>(),
        BTreeSet::from([a.clone(), d, candidate])
    );
    assert!(
        causes
            .values()
            .all(|cause| matches!(cause, Removal::Capacity))
    );
    let old_totals = aggregates(&original, config().max_ancestors_count).unwrap();
    let (old_ancestors, old_descendants) = old_totals.get(&a).unwrap();
    let old_snapshot =
        snapshot(original.get(&a).unwrap(), *old_ancestors, *old_descendants).unwrap();
    assert_eq!(old_snapshot.descendants_count, 4);
    assert_eq!(old_snapshot.descendants_fee.as_u64(), 110);
    let totals = aggregates(&entries, config().max_ancestors_count).unwrap();
    assert_eq!(totals.get(&a).unwrap().1.count, 2);
    assert_eq!(totals.get(&a).unwrap().1.fee, 100);
}

#[test]
fn trim_late_protected_rejection_keeps_the_original_notification_graph() {
    let (store, original, [a, b, c, d, e, candidate]) = rank_fixture();
    let mut entries = original.clone();
    let mut removed = BTreeSet::new();
    let mut causes = BTreeMap::new();
    let before = aggregates(&original, config().max_ancestors_count).unwrap();
    let result = trim_virtual(
        &mut entries,
        &store.snapshot().1,
        &config(),
        &mut removed,
        &mut causes,
        &BTreeSet::new(),
        &candidate,
        item_limit(0),
    );
    assert!(matches!(result, Err(Error::Rejected(Reject::Full(_)))));
    assert_eq!(removed, BTreeSet::from([a, b, c, d, e]));
    assert_eq!(entries.keys().cloned().collect::<Vec<_>>(), vec![candidate]);
    assert_eq!(
        aggregates(&original, config().max_ancestors_count).unwrap(),
        before
    );
    assert_eq!(store.capture(true).2.len(), 6);
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
    let mut entries = [a, b, d, e, candidate]
        .into_iter()
        .map(|entry| (entry.hash(), entry))
        .collect();
    let mut removed = BTreeSet::new();
    trim_virtual(
        &mut entries,
        &store.snapshot().1,
        &config(),
        &mut removed,
        &mut BTreeMap::new(),
        &BTreeSet::new(),
        &candidate_hash,
        item_limit(3),
    )
    .unwrap();
    assert_eq!(removed, expected);
}
