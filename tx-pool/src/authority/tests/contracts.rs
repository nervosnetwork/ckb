use super::super::{
    budget::owner_amount,
    chain::{self, ClearScope},
    ingress,
    model::{
        Accepted, Error, FullReason, Phase, Premises, RecoveryTriggers, Resolved, Source, Status,
        VerifiedScripts,
    },
    queue::{WorkSelection, WorkStage},
    store::Store,
};
use super::common::*;
use ckb_types::{
    bytes::Bytes,
    core::{
        Capacity,
        cell::{CellMeta, ResolvedTransaction},
    },
    packed::{Byte32, OutPoint},
    prelude::Entity,
};
use std::{collections::BTreeSet, sync::Arc};

#[test]
fn proposal_handoff_preserves_new_owner_in_either_edit_order() {
    // Synthetic identities exercise the index contract without searching for
    // transaction hash collisions or changing the public ingress policy.
    for (old_suffix, new_suffix) in [(1, 2), (2, 1)] {
        let store = store();
        let identity = |suffix| {
            let mut bytes = [7; 32];
            bytes[31] = suffix;
            Byte32::new(bytes)
        };
        let old = entry(
            &store,
            tx(6001).fake_hash(identity(old_suffix)),
            Source::Local,
        );
        let new = entry(
            &store,
            tx(6002).fake_hash(identity(new_suffix)),
            Source::Local,
        );
        let proposal = old.proposal();
        assert_eq!(proposal, new.proposal());
        assert!(matches!(
            commit_pairs(
                &store,
                vec![
                    (None, Some(Arc::clone(&old))),
                    (None, Some(Arc::clone(&new)))
                ]
            ),
            Err(Error::Full(FullReason::Other(
                "proposal short-ID collision"
            )))
        ));
        assert!(store.point(&old.hash()).1.is_none());
        assert!(store.point(&new.hash()).1.is_none());
        assert!(
            store
                .compact_lookup(std::slice::from_ref(&proposal))
                .1
                .is_empty()
        );
        insert(&store, Arc::clone(&old));
        assert!(matches!(
            commit_pairs(&store, vec![(None, Some(Arc::clone(&new)))]),
            Err(Error::Full(FullReason::Other(
                "proposal short-ID collision"
            )))
        ));
        assert!(store.point(&new.hash()).1.is_none());
        commit_pairs(
            &store,
            vec![
                (Some(Arc::clone(&old)), None),
                (None, Some(Arc::clone(&new))),
            ],
        )
        .unwrap();
        let (_, live, committed) = store.compact_lookup(std::slice::from_ref(&proposal));
        assert!(committed.is_empty());
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].0, proposal);
        assert!(Arc::ptr_eq(&live[0].1, &new));
        assert!(store.point(&old.hash()).1.is_none());
        assert!(Arc::ptr_eq(&store.point(&new.hash()).1.unwrap(), &new));
        delete(&store, new).unwrap();
        assert!(store.compact_lookup(&[proposal]).1.is_empty());
    }
}

#[test]
fn proposal_collision_rejection_preserves_incumbent_between_new_owners() {
    let store = store();
    let identity = |suffix| {
        let mut bytes = [8; 32];
        bytes[31] = suffix;
        Byte32::new(bytes)
    };
    let first = entry(&store, tx(6003).fake_hash(identity(1)), Source::Local);
    let middle = entry(&store, tx(6004).fake_hash(identity(2)), Source::Local);
    let last = entry(&store, tx(6005).fake_hash(identity(3)), Source::Local);
    let proposal = middle.proposal();
    assert_eq!(first.proposal(), proposal);
    assert_eq!(last.proposal(), proposal);
    insert(&store, Arc::clone(&middle));
    assert!(matches!(
        commit_pairs(
            &store,
            vec![
                (Some(Arc::clone(&middle)), None),
                (None, Some(Arc::clone(&first))),
                (None, Some(Arc::clone(&last))),
            ],
        ),
        Err(Error::Full(FullReason::Other(
            "proposal short-ID collision"
        )))
    ));
    assert!(store.point(&first.hash()).1.is_none());
    assert!(store.point(&last.hash()).1.is_none());
    assert!(Arc::ptr_eq(
        &store.point(&middle.hash()).1.unwrap(),
        &middle
    ));
    let (_, live, committed) = store.compact_lookup(&[proposal]);
    assert!(committed.is_empty());
    assert_eq!(live.len(), 1);
    assert!(Arc::ptr_eq(&live[0].1, &middle));
    assert!(!store.is_faulted());
}

#[test]
fn original_owner_identity_rejects_old_removal_after_same_hash_reentry() {
    let store = store();
    let original = entry(&store, tx(1), Source::Local);
    insert(&store, Arc::clone(&original));
    delete(&store, Arc::clone(&original)).unwrap();
    let replacement = entry(&store, tx(1), Source::Recovery);
    insert(&store, Arc::clone(&replacement));
    assert!(matches!(
        commit_pairs(&store, vec![(Some(Arc::clone(&original)), None)]),
        Err(Error::Stale)
    ));
    assert!(Arc::ptr_eq(
        &store.point(&original.hash()).1.unwrap(),
        &replacement
    ));
    assert!(!store.is_faulted());
}

#[test]
fn premises_cannot_replace_an_original_owner_observation() {
    let store = store();
    let old = entry(&store, tx(2), Source::Local);
    insert(&store, Arc::clone(&old));
    let mut premises = Premises::default();
    store.get(&old.hash(), &mut premises).unwrap();
    let replacement = replace(&store, Arc::clone(&old), Phase::Resolve);
    assert!(matches!(
        premises.observe_owner(&old.hash(), Some(&replacement)),
        Err(Error::Stale)
    ));
    assert!(matches!(
        store.get(&old.hash(), &mut premises),
        Err(Error::Stale)
    ));
}

#[test]
fn checking_an_owner_observation_preserves_conflicts() {
    let store = store();
    let old = entry(&store, tx(7823), Source::Local);
    let successor = entry(&store, tx(7823), Source::Recovery);
    let hash = old.hash();
    for (name, recorded, earlier, stale) in [
        ("unobserved absence", None, None, false),
        ("unobserved owner", None, Some(&old), false),
        ("same absence", Some(None), None, false),
        ("earlier presence", Some(None), Some(&old), true),
        ("earlier absence", Some(Some(&old)), None, true),
        ("same identity", Some(Some(&old)), Some(&old), false),
        (
            "different identity",
            Some(Some(&successor)),
            Some(&old),
            true,
        ),
    ] {
        let mut premises = Premises::default();
        if let Some(owner) = recorded {
            premises.observe_owner(&hash, owner).unwrap();
        }
        let checked = premises.check_owner(&hash, earlier);
        if stale {
            assert!(matches!(checked, Err(Error::Stale)), "{name}");
        } else {
            checked.unwrap();
        }
    }
}

#[test]
fn checking_an_owner_neither_adds_nor_replaces_verification_premises() {
    let store = store();
    let old = entry(&store, tx(7824), Source::Local);
    let successor = entry(&store, tx(7824), Source::Recovery);
    let hash = old.hash();
    let view = store.snapshot().0;
    let empty = Premises::default();
    empty.check_owner(&hash, Some(&old)).unwrap();
    // Checking an unobserved owner must not add a positive premise.
    store.check_premises(view, &empty).unwrap();
    empty.check_owner(&hash, None).unwrap();
    insert(&store, Arc::clone(&old));
    // Nor may it add an absent-owner premise that rejects this insertion.
    store.check_premises(view, &empty).unwrap();

    let mut premises = Premises::default();
    premises.observe_owner(&hash, Some(&old)).unwrap();
    assert!(matches!(
        premises.check_owner(&hash, Some(&successor)),
        Err(Error::Stale)
    ));
    assert!(matches!(
        premises.check_owner(&hash, None),
        Err(Error::Stale)
    ));
    store.check_premises(view, &premises).unwrap();
    replace(&store, old, Phase::Resolve);
    assert!(matches!(
        store.check_premises(view, &premises),
        Err(Error::Stale)
    ));
}

#[test]
fn checking_a_retired_owner_keeps_its_original_identity() {
    let store = store();
    let old = entry(&store, tx(7825), Source::Local);
    let hash = old.hash();
    let weak = Arc::downgrade(&old);
    let mut premises = Premises::default();
    premises.observe_owner(&hash, Some(&old)).unwrap();
    drop(old);
    assert!(weak.upgrade().is_none());
    let successor = entry(&store, tx(7825), Source::Recovery);
    for earlier in [None, Some(&successor)] {
        assert!(matches!(
            premises.check_owner(&hash, earlier),
            Err(Error::Stale)
        ));
    }
}

#[test]
fn premises_preserve_the_original_missing_spender() {
    let store = store();
    let point = OutPoint::new(tx(7802).hash(), 0);
    let mut original = Premises::default();
    assert!(store.spender(&point, &mut original).unwrap().is_none());
    let hash = accept(
        &store,
        spend(7803, std::slice::from_ref(&point), &[]),
        1,
        1,
        Status::Pending,
    );
    let mut fresh = Premises::default();
    assert_eq!(
        store.spender(&point, &mut fresh).unwrap(),
        Some(hash.clone())
    );
    assert!(matches!(
        store.spender(&point, &mut original),
        Err(Error::Stale)
    ));
    assert!(matches!(
        store.check_premises(store.snapshot().0, &original),
        Err(Error::Stale)
    ));

    delete(&store, store.point(&hash).1.unwrap()).unwrap();
    assert!(store.spender(&point, &mut original).unwrap().is_none());
    assert!(store.check_premises(store.snapshot().0, &original).is_ok());
}

#[test]
fn absence_is_a_current_fact_and_allows_absent_present_absent_history() {
    let store = store();
    let original = entry(&store, tx(3), Source::Local);
    let mut premises = Premises::default();
    assert!(
        store
            .get(&original.hash(), &mut premises)
            .unwrap()
            .is_none()
    );
    insert(&store, Arc::clone(&original));
    delete(&store, original).unwrap();
    assert!(store.check_premises(store.snapshot().0, &premises).is_ok());
}

#[test]
fn producer_premises_do_not_pin_retired_transaction_payloads() {
    let store = store();
    let source: Arc<[u8]> = vec![7; 128 * 1024].into();
    let backing = Arc::downgrade(&source);
    let hash = Byte32::new_unchecked(Bytes::from_owner(source).slice(1024..1056));
    let owner = entry(&store, tx(4).fake_hash(hash), Source::Local);
    let weak = Arc::downgrade(&owner);
    insert(&store, Arc::clone(&owner));
    let mut premises = Premises::default();
    store.get(&owner.hash(), &mut premises).unwrap();
    assert_eq!(backing.strong_count(), 1, "the fixture retains its source");
    delete(&store, owner).unwrap();
    assert!(weak.upgrade().is_none());
    assert_eq!(
        backing.strong_count(),
        0,
        "premises must release the source"
    );
    assert!(matches!(
        store.check_premises(store.snapshot().0, &premises),
        Err(Error::Stale)
    ));

    let successor = entry(
        &store,
        tx(4).fake_hash(Byte32::new([7; 32])),
        Source::Recovery,
    );
    insert(&store, successor);
    assert!(matches!(
        store.check_premises(store.snapshot().0, &premises),
        Err(Error::Stale)
    ));
}

#[test]
fn generation_clear_invalidates_old_premises_even_when_the_snapshot_is_identical() {
    let store = store();
    let (view, snapshot) = store.snapshot();
    chain::commit_clear(&store, Some(Arc::clone(&snapshot)), ClearScope::All).unwrap();
    assert_eq!(store.snapshot().1.tip_hash(), snapshot.tip_hash());
    assert!(matches!(
        store.check_premises(view, &Premises::default()),
        Err(Error::Stale)
    ));
    assert!(!store.is_faulted());
}

#[test]
fn rejected_multi_owner_edit_has_no_visible_prefix_or_accounting_change() {
    let store = store();
    let a = entry(&store, tx(9), Source::Local);
    let b = entry(&store, tx(10), Source::Local);
    insert(&store, Arc::clone(&a));
    insert(&store, Arc::clone(&b));
    let newest = replace(&store, Arc::clone(&b), Phase::Resolve);
    assert!(matches!(
        commit_pairs(&store, vec![(Some(Arc::clone(&a)), None), (Some(b), None)]),
        Err(Error::Stale)
    ));
    assert!(Arc::ptr_eq(&store.point(&a.hash()).1.unwrap(), &a));
    assert!(Arc::ptr_eq(
        &store.point(&newest.hash()).1.unwrap(),
        &newest
    ));
    assert_eq!(store.capture_all().owners.len(), 2);
    assert!(!store.budget.faulted());
}

#[test]
fn competing_exact_victim_replacement_consumes_credit_once() {
    let store = store();
    let hash = accept(&store, tx(11), 1, 1, Status::Pending);
    let before = store.point(&hash).1.unwrap();
    let after = before.with_phase(Phase::Resolve);
    commit_pairs(
        &store,
        vec![(Some(Arc::clone(&before)), Some(Arc::clone(&after)))],
    )
    .unwrap();
    assert!(matches!(
        commit_pairs(&store, vec![(Some(before), Some(Arc::clone(&after)))]),
        Err(Error::Stale)
    ));
    assert_eq!(store.accepted_usage().items, 0);
    assert!(Arc::ptr_eq(&store.point(&hash).1.unwrap(), &after));
    assert!(!store.budget.faulted());
}

#[test]
fn phase_edge_charges_match_owner_dependencies() {
    let store = store();
    let input = OutPoint::new(Byte32::new([1; 32]), 0);
    let dependency = OutPoint::new(Byte32::new([2; 32]), 0);
    let group = OutPoint::new(Byte32::new([3; 32]), 0);
    let header = Byte32::new([4; 32]);
    let transaction = spend(
        7800,
        std::slice::from_ref(&input),
        &[dependency.clone(), group.clone()],
    )
    .as_advanced_builder()
    .header_dep(header)
    .build();
    let owner = entry(&store, transaction.clone(), Source::Local);
    let cell = |point| CellMeta {
        out_point: point,
        ..CellMeta::default()
    };
    let resolved = Arc::new(ResolvedTransaction {
        transaction,
        resolved_inputs: vec![cell(input.clone())],
        resolved_cell_deps: vec![
            cell(dependency.clone()),
            cell(input.clone()),
            cell(dependency.clone()),
        ],
        resolved_dep_groups: vec![cell(group)],
    });
    let missing = BTreeSet::from([dependency]);
    let phases = [
        (Phase::Resolve, 4),
        (
            Phase::Verify(Arc::new(Resolved {
                transaction: Arc::clone(&resolved),
                fee: Capacity::shannons(1),
                view: store.snapshot().0,
                premises: Premises::default(),
                pool_cells: BTreeSet::new(),
            })),
            4,
        ),
        (
            Phase::Accepted(Accepted {
                transaction: resolved,
                scripts: VerifiedScripts::Fixture(1),
                fee: Capacity::shannons(1),
                size: owner.transaction.data().serialized_size_in_block(),
                timestamp: 0,
                parents: BTreeSet::from([input.tx_hash()]),
                context_sensitive: false,
                forced_status: Some(Status::Pending),
            }),
            5,
        ),
        (Phase::Waiting(missing.clone()), 4),
        (Phase::Replaced(RecoveryTriggers::RetryInputs(missing)), 4),
    ];
    for (phase, edges) in phases {
        let value = owner.with_phase(phase);
        assert_eq!(owner_amount(&value).edges, edges);
    }
}

#[test]
fn uncompleted_current_job_faults_but_stale_job_cancellation_is_clean() {
    let store = store();
    let owner = entry(&store, tx(13), Source::Local);
    insert(&store, owner);
    let job = store
        .pop(WorkStage::Resolve, WorkSelection::Any)
        .unwrap()
        .unwrap();
    chain::commit_clear(&store, None, ClearScope::All).unwrap();
    drop(job);
    assert!(!store.is_faulted());
    insert(&store, entry(&store, tx(14), Source::Local));
    let job = store
        .pop(WorkStage::Resolve, WorkSelection::Any)
        .unwrap()
        .unwrap();
    drop(job);
    assert!(store.is_faulted());
}

#[test]
fn selected_job_cannot_consume_promoted_work() {
    let store = store();
    let owner = entry(&store, tx(15), remote(1, 1));
    insert(&store, Arc::clone(&owner));
    let selected = store
        .pop(WorkStage::Resolve, WorkSelection::Any)
        .unwrap()
        .unwrap();
    let promoted = owner.with_source(Source::Recovery);
    commit_pairs(&store, vec![(Some(owner), Some(Arc::clone(&promoted)))]).unwrap();
    assert!(!selected.current());
    let successor = store
        .pop(WorkStage::Resolve, WorkSelection::Any)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&successor.entry, &promoted));
    assert!(
        store
            .pop(WorkStage::Resolve, WorkSelection::Any)
            .unwrap()
            .is_none()
    );
    delete(&store, promoted).unwrap();
    drop((selected, successor));
    assert!(!store.is_faulted());
}

#[test]
fn selection_and_identical_owner_edits_preserve_one_job_without_rewriting_facts() {
    let store = store();
    let owner = entry(&store, tx(902), Source::Local);
    insert(&store, Arc::clone(&owner));
    commit_pairs(
        &store,
        vec![(Some(Arc::clone(&owner)), Some(Arc::clone(&owner)))],
    )
    .unwrap();
    let job = store
        .pop(WorkStage::Resolve, WorkSelection::Any)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&job.entry, &owner));
    assert!(
        store
            .pop(WorkStage::Resolve, WorkSelection::Any)
            .unwrap()
            .is_none()
    );
    commit_pairs(
        &store,
        vec![(Some(Arc::clone(&owner)), Some(Arc::clone(&owner)))],
    )
    .unwrap();
    assert!(
        store
            .pop(WorkStage::Resolve, WorkSelection::Any)
            .unwrap()
            .is_none()
    );
    delete(&store, owner).unwrap();
    drop(job);
    assert!(!store.is_faulted());
    assert!(!store.budget.faulted());
}

#[test]
fn identical_owner_edit_still_rejects_a_stale_observation() {
    let store = store();
    let owner = entry(&store, tx(903), Source::Local);
    insert(&store, Arc::clone(&owner));
    let successor = replace(&store, Arc::clone(&owner), Phase::Resolve);
    assert!(matches!(
        commit_pairs(&store, vec![(Some(Arc::clone(&owner)), Some(owner))]),
        Err(Error::Stale)
    ));
    let selected = store
        .pop(WorkStage::Resolve, WorkSelection::Any)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&selected.entry, &successor));
    delete(&store, successor).unwrap();
    drop(selected);
    assert!(!store.is_faulted());
}

#[test]
fn peer_quota_enforces_limit_and_failed_reservation_exposes_no_owner() {
    let configuration = config();
    let store = store_with_pipeline_limit(chain_snapshot(), &configuration, 1_000_000);
    let source = remote(199, 1);
    let make = |nonce| {
        Arc::new(funded_tx(
            OutPoint::new(tx(nonce).hash(), 0),
            20_000_000_000,
        ))
    };
    let mut last = None;
    let mut full = false;
    for nonce in 7000..8000 {
        let transaction = make(nonce);
        match ingress::commit_ingress(&store, Arc::clone(&transaction), source) {
            Ok(_) => last = store.point(&transaction.hash()).1,
            Err(Error::Full(FullReason::Other("peer pipeline"))) => {
                full = true;
                break;
            }
            Err(error) => panic!("unexpected quota error: {error:?}"),
        }
    }
    assert!(full, "fixture reaches the configured peer bound");
    delete(&store, last.unwrap()).unwrap();
    let first = make(8000);
    let second = make(8001);
    ingress::commit_ingress(&store, Arc::clone(&first), source).unwrap();
    assert!(matches!(
        ingress::commit_ingress(&store, Arc::clone(&second), source),
        Err(Error::Full(FullReason::Other("peer pipeline")))
    ));
    assert!(store.point(&second.hash()).1.is_none());
    delete(&store, store.point(&first.hash()).1.unwrap()).unwrap();
    ingress::commit_ingress(&store, Arc::clone(&second), source).unwrap();
    assert!(store.point(&second.hash()).1.is_some());
    assert!(!store.is_faulted());
}

#[test]
fn worker_notifications_require_queued_work_or_returned_capacity() {
    use std::{
        future::Future,
        task::{Context, Waker},
    };
    let store = store();
    let mut context = Context::from_waker(Waker::noop());
    let mut capacity = std::pin::pin!(store.budget.active_changed.notified());
    let mut work = std::pin::pin!(store.work.notified());
    let mut maintenance = std::pin::pin!(store.changed.notified());
    capacity.as_mut().enable();
    work.as_mut().enable();
    maintenance.as_mut().enable();

    // An empty owner commit makes neither queued work nor additional capacity
    // available to sleeping workers.
    commit_pairs(&store, vec![]).unwrap();
    assert!(capacity.as_mut().poll(&mut context).is_pending());
    assert!(work.as_mut().poll(&mut context).is_pending());
    assert!(maintenance.as_mut().poll(&mut context).is_pending());

    let candidate = entry(&store, tx(901), Source::Local);
    insert(&store, Arc::clone(&candidate));
    assert!(work.as_mut().poll(&mut context).is_ready());
    assert!(capacity.as_mut().poll(&mut context).is_pending());
    assert!(maintenance.as_mut().poll(&mut context).is_pending());

    let mut work = std::pin::pin!(store.work.notified());
    work.as_mut().enable();
    let mut job = store
        .pop(WorkStage::Resolve, WorkSelection::Any)
        .unwrap()
        .unwrap();
    assert!(work.as_mut().poll(&mut context).is_pending());
    assert!(capacity.as_mut().poll(&mut context).is_pending());
    assert!(maintenance.as_mut().poll(&mut context).is_pending());

    delete(&store, Arc::clone(&job.entry)).unwrap();
    assert!(capacity.as_mut().poll(&mut context).is_pending());
    assert!(work.as_mut().poll(&mut context).is_pending());
    assert!(maintenance.as_mut().poll(&mut context).is_ready());
    job.mark_handled();
    drop(job);
    assert!(capacity.as_mut().poll(&mut context).is_ready());

    let mut maintenance = std::pin::pin!(store.changed.notified());
    chain::commit_clear(&store, Some(store.snapshot().1), ClearScope::All).unwrap();
    assert!(work.as_mut().poll(&mut context).is_ready());
    assert!(maintenance.as_mut().poll(&mut context).is_ready());
}

#[test]
fn work_broadcasts_are_observed_before_the_waiter_is_first_polled() {
    use std::{
        future::Future,
        task::{Context, Waker},
    };

    let changes: [fn(&Store); 4] = [
        |store| insert(store, entry(store, tx(902), Source::Local)),
        |store| {
            chain::commit_clear(store, Some(store.snapshot().1), ClearScope::All).unwrap();
        },
        Store::stop,
        Store::fault,
    ];
    for change in changes {
        let store = store();
        let mut work = std::pin::pin!(store.work.notified());
        change(&store);
        assert!(
            work.as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_ready()
        );
    }
}

#[test]
fn maintenance_stays_asleep_when_resolution_only_queues_verification() {
    use std::{future::Future, task::Context, task::Waker};

    let store = store();
    let verified = queued(&store, 904, remote(1, 1), 1);
    let unresolved = verified.with_phase(Phase::Resolve);
    let mut changed = std::pin::pin!(store.changed.notified());
    let mut context = Context::from_waker(Waker::noop());
    assert!(changed.as_mut().poll(&mut context).is_pending());
    insert(&store, Arc::clone(&unresolved));
    assert!(changed.as_mut().poll(&mut context).is_pending());
    replace(&store, unresolved, verified.phase.clone());
    assert!(changed.as_mut().poll(&mut context).is_pending());
}
