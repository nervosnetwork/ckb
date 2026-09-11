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

#[test]
fn replacement_history_skips_its_blocked_creation_event_and_observes_the_next_change() {
    for (old_nonce, new_nonce) in [(6401, 6402), (6402, 6401)] {
        for existing_waiter in [false, true] {
            let store = store();
            let configuration = ckb_app_config::TxPoolConfig {
                min_rbf_rate: ckb_types::core::FeeRate::from_u64(1_000),
                ..config()
            };
            let parent = accept(&store, output_tx(6400), 1, 1, Status::Pending);
            let point = OutPoint::new(parent, 0);
            let key = DependencyKey::Cell(point.clone());
            let old = accept(
                &store,
                spend(old_nonce, std::slice::from_ref(&point), &[]),
                1_000,
                1,
                Status::Pending,
            );
            let waiter = entry(&store, tx(6403), remote(1, 1))
                .with_phase(Phase::Waiting(BTreeSet::from([key.clone()])));
            if existing_waiter {
                insert(&store, Arc::clone(&waiter));
            }
            let replacement = entry(&store, spend(new_nonce, &[point], &[]), Source::Local);
            let (plan, reject) = admission(
                &store,
                &replacement,
                10_000,
                1,
                Status::Pending,
                &configuration,
            )
            .unwrap();
            assert!(reject.is_none());
            store.apply(plan).unwrap();
            let history = store.point(&old).1.unwrap();
            assert!(matches!(
                history.phase,
                Phase::Replaced {
                    require_all: true,
                    ..
                }
            ));
            let mut cursor = None;
            if existing_waiter {
                let page = store.wake_page(&mut cursor).unwrap();
                assert_eq!(page.key, key);
                assert_eq!(page.hashes, vec![waiter.hash()]);
                assert_eq!(drain_wakes(&store), 1);
            } else {
                assert!(store.wake_page(&mut cursor).is_none());
            }
            assert!(Arc::ptr_eq(&store.point(&old).1.unwrap(), &history));

            let current = store.point(&replacement.hash()).1.unwrap();
            store.apply(delete(&store, current)).unwrap();
            assert_eq!(drain_wakes(&store), 1);
            let recovered = store.point(&old).1.unwrap();
            assert_eq!(recovered.source, Source::Recovery);
            assert!(matches!(recovered.phase, Phase::Resolve));
            if existing_waiter {
                assert!(matches!(
                    store.point(&waiter.hash()).1.unwrap().phase,
                    Phase::Resolve
                ));
            }
            assert!(!store.is_faulted());
        }
    }
}

#[test]
fn replacement_history_creation_without_a_spender_preserves_all_and_policy_only_rules() {
    for all in [false, true] {
        let store = store();
        let parent = accept(&store, output_tx(6410), 1, 1, Status::Pending);
        let point = OutPoint::new(parent, 0);
        let old = accept(
            &store,
            spend(6411, std::slice::from_ref(&point), &[]),
            1,
            1,
            Status::Pending,
        );
        let before = store.point(&old).1.unwrap();
        replace(
            &store,
            before,
            Phase::Replaced {
                triggers: BTreeSet::from([DependencyKey::Cell(point.clone())]),
                require_all: all,
            },
        );
        assert_eq!(drain_wakes(&store), usize::from(all));
        assert_eq!(
            matches!(store.point(&old).1.unwrap().phase, Phase::Resolve),
            all
        );
        if !all {
            // Consuming the skipped creation event must leave the next event
            // eligible even when it is the first page ever queued for this key.
            let next = accept(&store, spend(6412, &[point], &[]), 1, 1, Status::Pending);
            drain_wakes(&store);
            store
                .apply(delete(&store, store.point(&next).1.unwrap()))
                .unwrap();
            assert_eq!(drain_wakes(&store), 1);
            assert!(matches!(store.point(&old).1.unwrap().phase, Phase::Resolve));
        }
        assert!(!store.is_faulted());
    }
}

#[test]
fn wake_cursor_excludes_a_removed_boundary_and_waiters_added_during_its_pass() {
    let store = store();
    let first = output_tx(6450);
    let second = output_tx(6451);
    let keys = BTreeSet::from([
        DependencyKey::Cell(OutPoint::new(first.hash(), 0)),
        DependencyKey::Cell(OutPoint::new(second.hash(), 0)),
    ]);
    let mut expected = Vec::new();
    for nonce in 6500..6570 {
        let owner = entry(&store, tx(nonce), remote(1, 1)).with_phase(Phase::Waiting(keys.clone()));
        expected.push(owner.hash());
        insert(&store, owner);
    }
    expected.sort();
    accept(&store, first, 1, 1, Status::Pending);
    let first_page = store.wake_page(&mut None).unwrap();
    assert_eq!(first_page.hashes, expected[..32]);
    let boundary = first_page.hashes.last().unwrap().clone();
    let mut cursor = None;
    store
        .apply(waiting::wake(&store, &mut cursor).unwrap().unwrap())
        .unwrap();

    // Neither deleting the cursor's owner nor adding a later hash may reopen
    // an already consumed prefix or admit a waiter into an older pass.
    store
        .apply(delete(&store, store.point(&boundary).1.unwrap()))
        .unwrap();
    let late = (6600..7600)
        .map(|nonce| entry(&store, tx(nonce), remote(2, 1)))
        .find(|owner| owner.hash() > boundary)
        .unwrap()
        .with_phase(Phase::Waiting(keys));
    insert(&store, Arc::clone(&late));
    for hashes in expected[32..].chunks(32) {
        let page = store.wake_page(&mut None).unwrap();
        assert_eq!(page.hashes, hashes);
        store
            .apply(waiting::wake(&store, &mut cursor).unwrap().unwrap())
            .unwrap();
    }
    assert!(store.wake_page(&mut None).is_none());
    assert!(matches!(
        store.point(&late.hash()).1.unwrap().phase,
        Phase::Waiting(_)
    ));

    accept(&store, second, 1, 1, Status::Pending);
    assert!(drain_wakes(&store) >= 3);
    assert!(store.point(&boundary).1.is_none());
    assert!(matches!(
        store.point(&late.hash()).1.unwrap().phase,
        Phase::Resolve
    ));
    assert_eq!(
        store
            .capture(false)
            .2
            .iter()
            .filter(|entry| matches!(entry.phase, Phase::Resolve))
            .count(),
        70
    );
    assert!(!store.is_faulted());
}

#[test]
fn trusted_waiter_checks_later_missing_producers_after_an_unready_known_producer() {
    let store = store();
    let mut parents = [output_tx(7700), output_tx(7701)].map(|transaction| {
        (
            DependencyKey::Cell(OutPoint::new(transaction.hash(), 0)),
            entry(&store, transaction, Source::Recovery),
        )
    });
    parents.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, parent) in &parents {
        insert(&store, Arc::clone(parent));
    }
    let waiter = entry(&store, tx(7702), Source::Recovery).with_phase(Phase::Waiting(
        parents.iter().map(|(key, _)| key.clone()).collect(),
    ));
    insert(&store, Arc::clone(&waiter));
    let [(first_key, first), (_, last)] = parents;
    store.apply(delete(&store, last)).unwrap();

    // The first key remains legitimately blocked, but the later missing
    // producer makes trusted recovery terminal on this validated cut.
    let (_, snapshot) = store.snapshot();
    assert!(!waiting::available(&store, &snapshot, &first_key, &mut Default::default()).unwrap());
    assert!(store.point(&first.hash()).1.is_some());
    assert!(drain_wakes(&store) > 0);
    assert!(store.point(&waiter.hash()).1.is_none());
    assert!(!store.is_faulted());
}

#[test]
fn shared_wake_readiness_rejects_changed_producer_and_spender_before_commit() {
    for change_spender in [false, true] {
        let store = store();
        let parent = output_tx(7800);
        let point = OutPoint::new(parent.hash(), 0);
        let key = DependencyKey::Cell(point.clone());
        let waiters = [7801, 7802].map(|nonce| {
            let owner = entry(&store, tx(nonce), remote(1, 1))
                .with_phase(Phase::Waiting(BTreeSet::from([key.clone()])));
            insert(&store, Arc::clone(&owner));
            owner
        });
        let parent = accept(&store, parent, 1, 1, Status::Pending);

        // Prepare both decisions first, then change a premise before commit.
        // This explicit interleaving exercises reuse of the first observation.
        let plan = waiting::wake(&store, &mut None).unwrap().unwrap();
        assert_eq!(plan.edits.len(), 2);
        if change_spender {
            accept(&store, spend(7803, &[point], &[]), 1, 1, Status::Pending);
        } else {
            let before = store.point(&parent).1.unwrap();
            let successor = replace(&store, Arc::clone(&before), before.phase.clone());
            assert!(!Arc::ptr_eq(&before, &successor));
        }
        assert!(matches!(store.apply(plan), Err(Error::Stale)));
        for waiter in &waiters {
            assert!(Arc::ptr_eq(&store.point(&waiter.hash()).1.unwrap(), waiter));
        }
        assert!(drain_wakes(&store) > 0);
        for waiter in &waiters {
            let current = store.point(&waiter.hash()).1.unwrap();
            assert_eq!(matches!(current.phase, Phase::Waiting(_)), change_spender);
            assert_eq!(matches!(current.phase, Phase::Resolve), !change_spender);
        }
        assert!(!store.is_faulted());
    }
}

#[test]
fn wake_short_circuit_does_not_observe_an_unvisited_trigger() {
    let store = store();
    let mut parents = [output_tx(7810), output_tx(7811)].map(|transaction| {
        (
            DependencyKey::Cell(OutPoint::new(transaction.hash(), 0)),
            transaction,
        )
    });
    parents.sort_by(|a, b| a.0.cmp(&b.0));
    let [(first_key, first), (trigger_key, trigger)] = parents;
    let keys = BTreeSet::from([first_key, trigger_key.clone()]);
    let waiters = [7812, 7813].map(|nonce| {
        let owner = entry(&store, tx(nonce), remote(1, 1)).with_phase(Phase::Waiting(keys.clone()));
        insert(&store, Arc::clone(&owner));
        owner
    });
    let trigger = accept(&store, trigger, 1, 1, Status::Pending);

    let mut cursor = None;
    let mut plan = waiting::wake(&store, &mut cursor).unwrap().unwrap();
    assert_eq!(cursor, Some(trigger_key));
    assert!(plan.edits.is_empty());
    // Both waiters stop at the earlier missing key. A first observation of
    // the trigger can bind its successor, proving the old owner was not read.
    let before = store.point(&trigger).1.unwrap();
    let successor = replace(&store, Arc::clone(&before), before.phase.clone());
    assert!(!Arc::ptr_eq(&before, &successor));
    plan.reads.owner(&trigger, Some(&successor)).unwrap();
    // Replacing the producer still starts a newer wake pass. Its independent
    // cursor premise must reject the old page, despite the unvisited key.
    assert!(matches!(store.apply(plan), Err(Error::Stale)));
    assert!(drain_wakes(&store) > 0);
    for waiter in &waiters {
        assert!(Arc::ptr_eq(&store.point(&waiter.hash()).1.unwrap(), waiter));
    }
    accept(&store, first, 1, 1, Status::Pending);
    assert!(drain_wakes(&store) > 0);
    for waiter in &waiters {
        assert!(matches!(
            store.point(&waiter.hash()).1.unwrap().phase,
            Phase::Resolve
        ));
    }
    assert!(!store.is_faulted());
}
