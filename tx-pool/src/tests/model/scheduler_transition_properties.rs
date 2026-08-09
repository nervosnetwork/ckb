use super::{
    scheduler_quotient::{
        SchedulerRefinementCursors, SchedulerRefinementEntry, SchedulerRefinementOwner,
        SchedulerRefinementSource, SchedulerRefinementStage, SchedulerRefinementVerifyClass,
        SchedulerRefinementVerifyOrder,
    },
    scheduler_transition::{
        SchedulerOwnerPopulation, SchedulerOwnerRing, SchedulerProjectionChange,
        SchedulerProjectionError, SchedulerSetProjection,
    },
};
use std::collections::BTreeSet;

fn entry(transaction: u8, source: SchedulerRefinementSource) -> SchedulerRefinementEntry {
    SchedulerRefinementEntry {
        transaction,
        version: u16::from(transaction),
        arrival: u16::from(transaction),
        source,
        stage: SchedulerRefinementStage::Verify(SchedulerRefinementVerifyClass::Small),
        fee: u64::from(transaction),
        bytes: 1,
    }
}

#[test]
fn model_scheduler_batch_is_one_order_independent_set_transition() {
    let first = entry(1, SchedulerRefinementSource::Proposal);
    let second = entry(2, SchedulerRefinementSource::Remote(1));
    let replacement = SchedulerRefinementEntry {
        version: 3,
        stage: SchedulerRefinementStage::Resolve,
        ..second
    };
    let cursors = SchedulerRefinementCursors {
        resolve: Some(SchedulerRefinementOwner::Trusted),
        verify: Some(SchedulerRefinementOwner::Remote(1)),
    };
    let projection = SchedulerSetProjection::new(
        [first, second],
        SchedulerRefinementVerifyOrder::FeeRate,
        cursors,
    )
    .expect("the initial scheduler projection is unique");
    let remove = SchedulerProjectionChange {
        transaction: 1,
        expected: Some(first),
        after: None,
    };
    let replace = SchedulerProjectionChange {
        transaction: 2,
        expected: Some(second),
        after: Some(replacement),
    };
    let after_cursors = SchedulerRefinementCursors {
        resolve: Some(SchedulerRefinementOwner::Remote(1)),
        verify: cursors.verify,
    };
    let left = projection
        .plan_changes(&[remove, replace], after_cursors)
        .expect("the set transition is legal");
    let right = projection
        .plan_changes(&[replace, remove], after_cursors)
        .expect("caller order cannot change the scheduler set");
    assert_eq!(left, right);
    assert_eq!(left.verify_order(), SchedulerRefinementVerifyOrder::FeeRate);
    assert_eq!(left.cursors(), after_cursors);
    assert_eq!(left.entries().get(&2), Some(&replacement));
}

#[test]
fn model_scheduler_replace_rejects_stale_duplicate_and_identity_ambiguous_changes() {
    let first = entry(1, SchedulerRefinementSource::Proposal);
    let projection = SchedulerSetProjection::new(
        [first],
        SchedulerRefinementVerifyOrder::Arrival,
        SchedulerRefinementCursors::default(),
    )
    .expect("the initial scheduler projection is unique");
    let remove = SchedulerProjectionChange {
        transaction: 1,
        expected: Some(first),
        after: None,
    };
    assert_eq!(
        projection.plan_changes(&[remove, remove], SchedulerRefinementCursors::default()),
        Err(SchedulerProjectionError::DuplicateTransaction(1))
    );
    assert_eq!(
        projection.plan_changes(
            &[SchedulerProjectionChange {
                transaction: 1,
                expected: None,
                after: None,
            }],
            SchedulerRefinementCursors::default(),
        ),
        Err(SchedulerProjectionError::ExistingEntryMismatch(1))
    );
    assert_eq!(
        projection.plan_changes(
            &[SchedulerProjectionChange {
                transaction: 1,
                expected: Some(first),
                after: Some(entry(2, SchedulerRefinementSource::Proposal)),
            }],
            SchedulerRefinementCursors::default(),
        ),
        Err(SchedulerProjectionError::IdentityMismatch(1))
    );
}

#[test]
fn model_scheduler_overlay_eligibility_is_union_not_intersection_or_constant() {
    let remote = SchedulerRefinementOwner::Remote(1);
    let trusted = SchedulerRefinementOwner::Trusted;
    let ring = SchedulerOwnerRing::new(
        SchedulerOwnerPopulation::new([remote], [remote])
            .expect("small owners are a subset of all committed owners"),
        SchedulerOwnerPopulation::new([trusted], std::iter::empty())
            .expect("the overlay has no small-only owner"),
        None,
    );
    assert!(ring.overlay_owner_is_eligible(false, remote));
    assert!(ring.overlay_owner_is_eligible(false, trusted));
    assert!(ring.overlay_owner_is_eligible(true, remote));
    assert!(!ring.overlay_owner_is_eligible(true, trusted));
    assert!(!ring.overlay_owner_is_eligible(false, SchedulerRefinementOwner::Remote(2)));
}

#[test]
fn model_scheduler_owner_bound_and_next_after_skip_each_ineligible_owner_once() {
    let remote_one = SchedulerRefinementOwner::Remote(1);
    let remote_two = SchedulerRefinementOwner::Remote(2);
    let trusted = SchedulerRefinementOwner::Trusted;
    let ring = SchedulerOwnerRing::new(
        SchedulerOwnerPopulation::new([remote_one, trusted], [remote_one, trusted])
            .expect("the committed population is legal"),
        SchedulerOwnerPopulation::new([remote_two], [remote_two])
            .expect("the overlay population is legal"),
        Some(remote_one),
    );
    assert_eq!(ring.owner_bound(false), Some(3));
    assert_eq!(
        ring.first_available(false, &BTreeSet::from([remote_two])),
        Some(trusted)
    );
    assert_eq!(
        ring.first_available(false, &BTreeSet::from([remote_two, trusted, remote_one])),
        None
    );
}

#[test]
fn model_scheduler_duplicate_overlay_owner_is_a_bounded_probe_not_a_second_owner() {
    let remote = SchedulerRefinementOwner::Remote(1);
    let ring = SchedulerOwnerRing::new(
        SchedulerOwnerPopulation::new([remote], [remote])
            .expect("the committed population is legal"),
        SchedulerOwnerPopulation::new([remote], [remote]).expect("the overlay population is legal"),
        None,
    );
    assert_eq!(ring.owner_bound(false), Some(2));
    assert_eq!(ring.first_available(false, &BTreeSet::new()), Some(remote));
    assert_eq!(ring.first_available(false, &BTreeSet::from([remote])), None);
}
