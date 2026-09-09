use super::super::{
    model::{DependencyKey, Error, Phase, Source, Status},
    notice::Class,
    store::{Plan, Store},
    waiting,
};
use super::common::*;
use ckb_types::packed::OutPoint;
use std::{collections::BTreeSet, sync::Arc};

fn drain_wakes(store: &Store) -> usize {
    let mut cursor = None;
    for count in 0..100 {
        let Some(plan) = waiting::wake(store, &mut cursor).unwrap() else {
            return count;
        };
        store.apply(plan).unwrap();
    }
    panic!("wake passes failed to terminate");
}

#[test]
fn registering_missing_after_availability_rejects_the_old_absence() {
    let store = store();
    let parent = output_tx(16);
    let point = OutPoint::new(parent.hash(), 0);
    let owner = entry(
        &store,
        spend(17, std::slice::from_ref(&point), &[]),
        remote(1, 1),
    );
    insert(&store, Arc::clone(&owner));
    let mut plan = Plan::new(store.snapshot().0, Class::Remote);
    store.get(&parent.hash(), &mut plan.reads).unwrap();
    plan.edit(
        Some(Arc::clone(&owner)),
        Some(owner.with_phase(Phase::Waiting(BTreeSet::from([DependencyKey::Cell(point)])))),
    )
    .unwrap();
    accept(&store, parent, 1, 1, Status::Pending);
    assert!(matches!(store.apply(plan), Err(Error::Stale)));
    assert!(matches!(
        store.point(&owner.hash()).1.unwrap().phase,
        Phase::Resolve
    ));
}

#[test]
fn producer_availability_wakes_every_page_and_does_not_spin_on_a_second_missing_key() {
    use std::{future::Future, task::Context, task::Waker};

    let store = store();
    let first = output_tx(18);
    let second = output_tx(19);
    let a = DependencyKey::Cell(OutPoint::new(first.hash(), 0));
    let b = DependencyKey::Cell(OutPoint::new(second.hash(), 0));
    for nonce in 100..170 {
        let owner = entry(&store, tx(nonce), remote(1, 1))
            .with_phase(Phase::Waiting(BTreeSet::from([a.clone(), b.clone()])));
        insert(&store, owner);
    }
    let mut changed = std::pin::pin!(store.changed.notified());
    let mut context = Context::from_waker(Waker::noop());
    assert!(changed.as_mut().poll(&mut context).is_pending());
    accept(&store, first, 1, 1, Status::Pending);
    assert!(changed.as_mut().poll(&mut context).is_ready());
    assert!(drain_wakes(&store) >= 3);
    assert_eq!(
        store
            .capture(false)
            .2
            .iter()
            .filter(|entry| matches!(entry.phase, Phase::Waiting(_)))
            .count(),
        70
    );
    assert!(waiting::wake(&store, &mut None).unwrap().is_none());
    let mut changed = std::pin::pin!(store.changed.notified());
    assert!(changed.as_mut().poll(&mut context).is_pending());
    accept(&store, second, 1, 1, Status::Pending);
    assert!(changed.as_mut().poll(&mut context).is_ready());
    assert!(drain_wakes(&store) >= 3);
    assert_eq!(
        store
            .capture(false)
            .2
            .iter()
            .filter(|entry| matches!(entry.phase, Phase::Resolve))
            .count(),
        70
    );
    assert!(waiting::wake(&store, &mut None).unwrap().is_none());
}

#[test]
fn wake_selection_rotates_after_a_stale_page_before_retrying_its_key() {
    let store = store();
    let mut parents = [output_tx(6120), output_tx(6121)].map(|transaction| {
        (
            DependencyKey::Cell(OutPoint::new(transaction.hash(), 0)),
            transaction,
        )
    });
    parents.sort_by(|a, b| a.0.cmp(&b.0));
    let [(first_key, first), (second_key, second)] = parents;
    let first_waiter = entry(&store, tx(6122), remote(1, 1))
        .with_phase(Phase::Waiting(BTreeSet::from([first_key.clone()])));
    let second_waiter = entry(&store, tx(6123), remote(2, 1))
        .with_phase(Phase::Waiting(BTreeSet::from([second_key.clone()])));
    insert(&store, Arc::clone(&first_waiter));
    insert(&store, Arc::clone(&second_waiter));
    accept(&store, first, 1, 1, Status::Pending);
    accept(&store, second, 1, 1, Status::Pending);

    let mut cursor = None;
    let stale = waiting::wake(&store, &mut cursor).unwrap().unwrap();
    assert_eq!(cursor, Some(first_key.clone()));
    let successor = replace(
        &store,
        Arc::clone(&first_waiter),
        first_waiter.phase.clone(),
    );
    assert!(matches!(store.apply(stale), Err(Error::Stale)));
    assert!(Arc::ptr_eq(
        &store.point(&first_waiter.hash()).1.unwrap(),
        &successor
    ));

    let next = waiting::wake(&store, &mut cursor).unwrap().unwrap();
    assert_eq!(cursor, Some(second_key));
    store.apply(next).unwrap();
    assert!(matches!(
        store.point(&second_waiter.hash()).1.unwrap().phase,
        Phase::Resolve
    ));
    assert!(matches!(
        store.point(&first_waiter.hash()).1.unwrap().phase,
        Phase::Waiting(_)
    ));
    // The stale first page remains pending; wrapping does not need it to clear.
    assert!(waiting::wake(&store, &mut cursor).unwrap().is_some());
    assert_eq!(cursor, Some(first_key));
    assert!(!store.is_faulted());
}

#[test]
fn replacement_history_obeys_all_and_any_blockers_and_recovers_as_trusted() {
    let store = store();
    let first = output_tx(20);
    let second = output_tx(21);
    let keys = BTreeSet::from([
        DependencyKey::Cell(OutPoint::new(first.hash(), 0)),
        DependencyKey::Cell(OutPoint::new(second.hash(), 0)),
    ]);
    let all = entry(&store, tx(22), remote(1, 1)).with_phase(Phase::Replaced {
        triggers: keys.clone(),
        require_all: true,
    });
    let any = entry(&store, tx(23), remote(2, 1)).with_phase(Phase::Replaced {
        triggers: keys,
        require_all: false,
    });
    insert(&store, Arc::clone(&all));
    insert(&store, Arc::clone(&any));
    accept(&store, first, 1, 1, Status::Pending);
    drain_wakes(&store);
    assert!(matches!(
        store.point(&all.hash()).1.unwrap().phase,
        Phase::Replaced { .. }
    ));
    let recovered = store.point(&any.hash()).1.unwrap();
    assert_eq!(recovered.source, Source::Recovery);
    assert!(matches!(recovered.phase, Phase::Resolve));
    accept(&store, second, 1, 1, Status::Pending);
    drain_wakes(&store);
    assert_eq!(store.point(&all.hash()).1.unwrap().source, Source::Recovery);
}

#[test]
fn trusted_waiter_is_retired_when_its_known_pending_producer_terminalizes() {
    for source in [Source::Recovery, Source::Proposal { remote: None }] {
        let store = store();
        let parent = entry(&store, output_tx(9010), source);
        let point = OutPoint::new(parent.hash(), 0);
        insert(&store, Arc::clone(&parent));
        let child = entry(
            &store,
            spend(9011, std::slice::from_ref(&point), &[]),
            source,
        )
        .with_phase(Phase::Waiting([DependencyKey::Cell(point)].into()));
        insert(&store, Arc::clone(&child));
        assert_eq!(drain_wakes(&store), 0);
        store.apply(delete(&store, parent)).unwrap();
        assert!(drain_wakes(&store) > 0);
        assert!(store.point(&child.hash()).1.is_none());
        assert!(!store.is_faulted());
    }
}
