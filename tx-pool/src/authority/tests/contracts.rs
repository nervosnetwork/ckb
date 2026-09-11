use super::super::{
    budget::{OwnerDelta, owner_amount},
    chain,
    model::{
        Accepted, DependencyKey, Entry, Error, FullReason, Phase, RelationKey, Resolved, Source,
        Status,
    },
    notice::Class,
    queue::WorkStage,
    store::{DEP, Plan, ReadSet},
};
use super::common::*;
use ckb_types::{
    core::{
        Capacity,
        cell::{CellMeta, ResolvedTransaction},
    },
    packed::{Byte32, OutPoint},
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
        let mut collision = Plan::new(store.snapshot().0, Class::Trusted, Default::default());
        collision.edit(None, Some(Arc::clone(&old)), None).unwrap();
        collision.edit(None, Some(Arc::clone(&new)), None).unwrap();
        assert!(matches!(
            store.apply(collision),
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
        let mut collision = Plan::new(store.snapshot().0, Class::Trusted, Default::default());
        collision.edit(None, Some(Arc::clone(&new)), None).unwrap();
        assert!(matches!(
            store.apply(collision),
            Err(Error::Full(FullReason::Other(
                "proposal short-ID collision"
            )))
        ));
        assert!(store.point(&new.hash()).1.is_none());
        let mut handoff = delete(&store, Arc::clone(&old));
        handoff.edit(None, Some(Arc::clone(&new)), None).unwrap();
        store.apply(handoff).unwrap();
        let (_, live, committed) = store.compact_lookup(std::slice::from_ref(&proposal));
        assert!(committed.is_empty());
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].0, proposal);
        assert!(Arc::ptr_eq(&live[0].1, &new));
        assert!(store.point(&old.hash()).1.is_none());
        assert!(Arc::ptr_eq(&store.point(&new.hash()).1.unwrap(), &new));
        store.apply(delete(&store, new)).unwrap();
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
    let mut plan = delete(&store, Arc::clone(&middle));
    plan.edit(None, Some(Arc::clone(&first)), None).unwrap();
    plan.edit(None, Some(Arc::clone(&last)), None).unwrap();
    assert!(matches!(
        store.apply(plan),
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
    let stale = delete(&store, Arc::clone(&original));
    store.apply(delete(&store, Arc::clone(&original))).unwrap();
    let replacement = entry(&store, tx(1), Source::Recovery);
    insert(&store, Arc::clone(&replacement));
    assert!(matches!(store.apply(stale), Err(Error::Stale)));
    assert!(Arc::ptr_eq(
        &store.point(&original.hash()).1.unwrap(),
        &replacement
    ));
    assert!(!store.is_faulted());
}

#[test]
fn a_read_set_cannot_replace_an_original_owner_observation() {
    let store = store();
    let old = entry(&store, tx(2), Source::Local);
    insert(&store, Arc::clone(&old));
    let mut reads = ReadSet::default();
    store.get(&old.hash(), &mut reads).unwrap();
    let replacement = replace(&store, Arc::clone(&old), Phase::Resolve);
    assert!(matches!(
        reads.observe_owner(&old.hash(), Some(&replacement)),
        Err(Error::Stale)
    ));
    let mut fresh = ReadSet::default();
    store.get(&old.hash(), &mut fresh).unwrap();
    assert!(matches!(reads.merge(&fresh), Err(Error::Stale)));
}

#[test]
fn merging_observations_preserves_the_original_missing_spender() {
    let store = store();
    let point = OutPoint::new(tx(7802).hash(), 0);
    let mut original = ReadSet::default();
    assert!(store.spender(&point, &mut original).unwrap().is_none());
    let hash = accept(
        &store,
        spend(7803, std::slice::from_ref(&point), &[]),
        1,
        1,
        Status::Pending,
    );
    let mut fresh = ReadSet::default();
    assert_eq!(
        store.spender(&point, &mut fresh).unwrap(),
        Some(hash.clone())
    );
    assert!(matches!(original.merge(&fresh), Err(Error::Stale)));
    assert!(matches!(
        store.read_selected(store.snapshot().0, &original, || ()),
        Err(Error::Stale)
    ));

    store
        .apply(delete(&store, store.point(&hash).1.unwrap()))
        .unwrap();
    let mut absent_again = ReadSet::default();
    assert!(store.spender(&point, &mut absent_again).unwrap().is_none());
    original.merge(&absent_again).unwrap();
    assert!(
        store
            .read_selected(store.snapshot().0, &original, || ())
            .is_ok()
    );
}

#[test]
fn absence_is_a_current_fact_and_allows_absent_present_absent_history() {
    let store = store();
    let original = entry(&store, tx(3), Source::Local);
    let mut reads = ReadSet::default();
    assert!(store.get(&original.hash(), &mut reads).unwrap().is_none());
    insert(&store, Arc::clone(&original));
    store.apply(delete(&store, original)).unwrap();
    assert!(
        store
            .read_selected(store.snapshot().0, &reads, || ())
            .is_ok()
    );
}

#[test]
fn producer_read_sets_do_not_pin_retired_transaction_payloads() {
    let store = store();
    let owner = entry(&store, tx(4), Source::Local);
    let weak = Arc::downgrade(&owner);
    insert(&store, Arc::clone(&owner));
    let mut reads = ReadSet::default();
    store.get(&owner.hash(), &mut reads).unwrap();
    store.apply(delete(&store, owner)).unwrap();
    assert!(weak.upgrade().is_none());
    assert!(matches!(
        store.read_selected(store.snapshot().0, &reads, || ()),
        Err(Error::Stale)
    ));
}

#[test]
fn accepted_capture_ignores_prepool_changes_but_detects_accepted_edits() {
    let store = store();
    accept(&store, tx(5), 1, 1, Status::Pending);
    let (view, _, _, reads) = store.capture(true);
    insert(&store, entry(&store, tx(6), Source::Local));
    assert!(store.read_selected(view, &reads, || ()).is_ok());
    accept(&store, tx(7), 1, 1, Status::Pending);
    assert!(matches!(
        store.read_selected(view, &reads, || ()),
        Err(Error::Stale)
    ));
}

#[test]
fn generation_clear_invalidates_old_reads_even_when_the_snapshot_is_identical() {
    let store = store();
    let (view, snapshot) = store.snapshot();
    let mut old = Plan::new(view, Class::Trusted, Default::default());
    old.edit(None, Some(entry(&store, tx(8), Source::Local)), None)
        .unwrap();
    store
        .apply(chain::clear(&store, Some(Arc::clone(&snapshot)), false).unwrap())
        .unwrap();
    assert_eq!(store.snapshot().1.tip_hash(), snapshot.tip_hash());
    assert!(matches!(store.apply(old), Err(Error::Stale)));
    assert!(!store.is_faulted());
}

#[test]
fn rejected_multi_owner_edit_has_no_visible_prefix_or_accounting_change() {
    let store = store();
    let a = entry(&store, tx(9), Source::Local);
    let b = entry(&store, tx(10), Source::Local);
    insert(&store, Arc::clone(&a));
    insert(&store, Arc::clone(&b));
    let mut plan = delete(&store, Arc::clone(&a));
    plan.edit(Some(Arc::clone(&b)), None, None).unwrap();
    let newest = replace(&store, b, Phase::Resolve);
    assert!(matches!(store.apply(plan), Err(Error::Stale)));
    assert!(Arc::ptr_eq(&store.point(&a.hash()).1.unwrap(), &a));
    assert!(Arc::ptr_eq(
        &store.point(&newest.hash()).1.unwrap(),
        &newest
    ));
    assert_eq!(store.capture(false).2.len(), 2);
    assert!(!store.budget.faulted());
}

#[test]
fn competing_exact_victim_replacement_consumes_credit_once() {
    let store = store();
    let hash = accept(&store, tx(11), 1, 1, Status::Pending);
    let before = store.point(&hash).1.unwrap();
    let after = before.with_phase(Phase::Resolve);
    let mut first = Plan::new(store.snapshot().0, Class::Trusted, Default::default());
    first
        .edit(Some(Arc::clone(&before)), Some(Arc::clone(&after)), None)
        .unwrap();
    let second = first.clone();
    store.apply(first).unwrap();
    assert!(matches!(store.apply(second), Err(Error::Stale)));
    assert_eq!(store.budget.accepted_usage().items, 0);
    assert!(Arc::ptr_eq(&store.point(&hash).1.unwrap(), &after));
    assert!(!store.budget.faulted());
}

#[test]
fn dependency_results_preserve_unique_order_and_phase_edge_charges() {
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
    .header_dep(header.clone())
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
        resolved_dep_groups: vec![cell(group.clone())],
    });
    let declared = vec![
        DependencyKey::Cell(input.clone()),
        DependencyKey::Cell(dependency.clone()),
        DependencyKey::Cell(group),
        DependencyKey::Header(header.clone()),
    ];
    let missing = BTreeSet::from([
        DependencyKey::Cell(dependency),
        DependencyKey::Header(header),
    ]);
    let missing_order: Vec<_> = missing.iter().cloned().collect();
    let phases = [
        (Phase::Resolve, declared.clone(), 4),
        (
            Phase::Verify(Arc::new(Resolved {
                transaction: Arc::clone(&resolved),
                fee: Capacity::shannons(1),
                view: store.snapshot().0,
                reads: ReadSet::default(),
                pool_cells: BTreeSet::new(),
            })),
            declared.clone(),
            4,
        ),
        (
            Phase::Accepted(Accepted {
                transaction: resolved,
                cycles: 1,
                fee: Capacity::shannons(1),
                size: owner.transaction.data().serialized_size_in_block(),
                timestamp: 0,
                parents: BTreeSet::from([input.tx_hash()]),
                context_sensitive: false,
                forced_status: Some(Status::Pending),
            }),
            declared.clone(),
            5,
        ),
        (Phase::Waiting(missing.clone()), missing_order.clone(), 4),
        (
            Phase::Replaced {
                triggers: missing,
                require_all: false,
            },
            missing_order,
            4,
        ),
    ];
    for (phase, expected, edges) in phases {
        let value = owner.with_phase(phase);
        assert_eq!(
            value
                .declared_dependencies()
                .into_iter()
                .collect::<Vec<_>>(),
            declared
        );
        assert_eq!(
            value.dependencies().into_iter().collect::<Vec<_>>(),
            expected
        );
        assert_eq!(owner_amount(&value).unwrap().edges, edges);
    }
}

#[test]
fn uncompleted_current_job_faults_but_stale_job_cancellation_is_clean() {
    let store = store();
    let owner = entry(&store, tx(13), Source::Local);
    insert(&store, owner);
    let job = store.pop(WorkStage::Resolve, false).unwrap().unwrap();
    store
        .apply(chain::clear(&store, None, false).unwrap())
        .unwrap();
    drop(job);
    assert!(!store.is_faulted());
    insert(&store, entry(&store, tx(14), Source::Local));
    let job = store.pop(WorkStage::Resolve, false).unwrap().unwrap();
    drop(job);
    assert!(store.is_faulted());
}

#[test]
fn selected_job_cannot_consume_promoted_work() {
    let store = store();
    let owner = entry(&store, tx(15), remote(1, 1));
    insert(&store, Arc::clone(&owner));
    let selected = store.pop(WorkStage::Resolve, false).unwrap().unwrap();
    let promoted = Arc::new(Entry {
        source: Source::Recovery,
        ..owner.as_ref().clone()
    });
    let mut plan = Plan::new(store.snapshot().0, Class::Trusted, Default::default());
    plan.edit(Some(owner), Some(Arc::clone(&promoted)), None)
        .unwrap();
    store.apply(plan).unwrap();
    assert!(!selected.current().unwrap());
    let successor = store.pop(WorkStage::Resolve, false).unwrap().unwrap();
    assert!(Arc::ptr_eq(&successor.entry, &promoted));
    assert!(store.pop(WorkStage::Resolve, false).unwrap().is_none());
    store.apply(delete(&store, promoted)).unwrap();
    drop((selected, successor));
    assert!(!store.is_faulted());
}

#[test]
fn selection_and_identical_owner_edits_preserve_one_job_without_rewriting_facts() {
    let store = store();
    let owner = entry(&store, tx(902), Source::Local);
    insert(&store, Arc::clone(&owner));
    let (view, _, _, reads) = store.capture(false);
    let mut unchanged = Plan::new(view, Class::Trusted, Default::default());
    unchanged
        .edit(Some(Arc::clone(&owner)), Some(Arc::clone(&owner)), None)
        .unwrap();
    store.apply(unchanged.clone()).unwrap();
    let job = store.pop(WorkStage::Resolve, false).unwrap().unwrap();
    assert!(Arc::ptr_eq(&job.entry, &owner));
    assert!(store.pop(WorkStage::Resolve, false).unwrap().is_none());
    store.apply(unchanged).unwrap();
    assert!(store.pop(WorkStage::Resolve, false).unwrap().is_none());
    assert!(store.read_selected(view, &reads, || ()).is_ok());
    store.apply(delete(&store, owner)).unwrap();
    drop(job);
    assert!(!store.is_faulted());
    assert!(!store.budget.faulted());
}

#[test]
fn identical_owner_edit_still_rejects_a_stale_observation() {
    let store = store();
    let owner = entry(&store, tx(903), Source::Local);
    insert(&store, Arc::clone(&owner));
    let mut unchanged = Plan::new(store.snapshot().0, Class::Trusted, Default::default());
    unchanged
        .edit(Some(Arc::clone(&owner)), Some(Arc::clone(&owner)), None)
        .unwrap();
    let successor = replace(&store, owner, Phase::Resolve);
    assert!(matches!(store.apply(unchanged), Err(Error::Stale)));
    let selected = store.pop(WorkStage::Resolve, false).unwrap().unwrap();
    assert!(Arc::ptr_eq(&selected.entry, &successor));
    store.apply(delete(&store, successor)).unwrap();
    drop(selected);
    assert!(!store.is_faulted());
}

#[test]
fn complete_dependency_observation_detects_late_readers() {
    let store = store();
    let point = OutPoint::new(Byte32::new([7; 32]), 0);
    let mut reads = ReadSet::default();
    assert!(
        store
            .members(
                &RelationKey::Dependency(DependencyKey::Cell(point.clone())),
                DEP,
                &mut reads,
            )
            .unwrap()
            .is_empty()
    );
    accept(&store, spend(24, &[], &[point]), 1, 1, Status::Pending);
    let plan = Plan::new(store.snapshot().0, Class::Trusted, reads);
    assert!(matches!(store.apply(plan), Err(Error::Stale)));
}

#[test]
fn chain_preparation_pauses_claims_and_owner_changes_until_release() {
    let store = store();
    insert(&store, entry(&store, tx(35), Source::Local));
    let pause = store.begin_chain().unwrap();
    assert!(store.pop(WorkStage::Resolve, false).unwrap().is_none());
    let mut plan = Plan::new(store.snapshot().0, Class::Trusted, Default::default());
    plan.edit(None, Some(entry(&store, tx(36), Source::Local)), None)
        .unwrap();
    assert!(matches!(
        store.apply(plan),
        Err(Error::Full(FullReason::ChainTransition))
    ));
    drop(pause);
    let mut selected = store.pop(WorkStage::Resolve, false).unwrap().unwrap();
    selected.complete();
}

#[test]
fn peer_quota_rechecks_disjoint_prepared_ingress_and_failed_reservation_exposes_no_owner() {
    use crate::authority::ingress;
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
        let plan = ingress::prepare(&store, Arc::clone(&transaction), source).unwrap();
        match store.apply(plan) {
            Ok(_) => last = store.point(&transaction.hash()).1,
            Err(Error::Full(FullReason::Other("peer pipeline"))) => {
                full = true;
                break;
            }
            Err(error) => panic!("unexpected quota error: {error:?}"),
        }
    }
    assert!(full, "fixture reaches the configured peer bound");
    store.apply(delete(&store, last.unwrap())).unwrap();
    let first = make(8000);
    let second = make(8001);
    let a = ingress::prepare(&store, Arc::clone(&first), source).unwrap();
    let b = ingress::prepare(&store, Arc::clone(&second), source).unwrap();
    store.apply(a).unwrap();
    assert!(matches!(
        store.apply(b.clone()),
        Err(Error::Full(FullReason::Other("peer pipeline")))
    ));
    assert!(store.point(&second.hash()).1.is_none());
    store
        .apply(delete(&store, store.point(&first.hash()).1.unwrap()))
        .unwrap();
    store.apply(b).unwrap();
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
    let mut capacity = std::pin::pin!(store.budget.changed.notified());
    let mut work = std::pin::pin!(store.work.notified());
    let mut maintenance = std::pin::pin!(store.changed.notified());
    capacity.as_mut().enable();
    work.as_mut().enable();
    maintenance.as_mut().enable();

    // A discarded zero-difference reservation and an empty owner commit make
    // neither queued work nor additional capacity available to sleeping workers.
    drop(
        OwnerDelta::new(std::iter::empty(), std::iter::empty())
            .unwrap()
            .reserve(&store.budget)
            .unwrap(),
    );
    store
        .apply(Plan::new(
            store.snapshot().0,
            Class::Trusted,
            Default::default(),
        ))
        .unwrap();
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
    let mut job = store.pop(WorkStage::Resolve, false).unwrap().unwrap();
    assert!(work.as_mut().poll(&mut context).is_pending());
    assert!(capacity.as_mut().poll(&mut context).is_pending());
    assert!(maintenance.as_mut().poll(&mut context).is_pending());

    store.apply(delete(&store, Arc::clone(&job.entry))).unwrap();
    assert!(capacity.as_mut().poll(&mut context).is_ready());
    assert!(work.as_mut().poll(&mut context).is_pending());
    assert!(maintenance.as_mut().poll(&mut context).is_ready());
    job.complete();

    let mut maintenance = std::pin::pin!(store.changed.notified());
    let mut lifecycle = Plan::new(store.snapshot().0, Class::Critical, Default::default());
    lifecycle.reset(store.snapshot().1, false);
    store.apply(lifecycle).unwrap();
    assert!(work.as_mut().poll(&mut context).is_ready());
    assert!(maintenance.as_mut().poll(&mut context).is_ready());
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
