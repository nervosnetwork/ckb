//! Refinement of fallible, bounded and read-only contractual observations.

use super::claim_relations::contract::{
    bounded_prefix_len, cumulative_effect_region_projection, exact_operational_projection,
    reservation_capacity_is_sufficient,
};
use super::foundation::{admit_remote_until, limits};
use crate::authority::{
    effect::{CommittedEffect, EffectLimits},
    plan::{TxPoolAuthority, test_support::retired_buffer_capacity_for_foundation},
    state::RemoteDeadline,
};
use std::num::NonZeroUsize;

#[test]
fn uak_plan_scratch_reservations_observe_capacity_without_authority_mutation() {
    let retired_capacity = retired_buffer_capacity_for_foundation(1)
        .expect("one retired slot is a legal fallible reservation");
    assert!(reservation_capacity_is_sufficient(1, retired_capacity));

    let mut authority = TxPoolAuthority::for_foundation(limits());
    let before = authority.normalized_snapshot();
    assert_eq!(
        authority
            .reserve_primary_owner_capacity_for_foundation(0)
            .expect("zero additional owners require no allocation"),
        0
    );
    let primary_capacity = authority
        .reserve_primary_owner_capacity_for_foundation(1)
        .expect("one primary owner slot is a legal fallible reservation");
    assert!(reservation_capacity_is_sufficient(1, primary_capacity));
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_remote_expiry_removes_exactly_the_effect_bounded_due_prefix() {
    let effect_limits =
        EffectLimits::for_foundation().with_remote_effects_per_batch_for_foundation(1);
    let mut authority = TxPoolAuthority::for_foundation_with_effect_limits(limits(), effect_limits)
        .expect("the one-effect Remote batch limit is valid");
    let first = admit_remote_until(&mut authority, 2_201, 201, 9);
    let second = admit_remote_until(&mut authority, 2_202, 202, 10);
    let third = admit_remote_until(&mut authority, 2_203, 203, 10);
    let caller_limit = NonZeroUsize::new(3).expect("three is non-zero");
    assert_eq!(bounded_prefix_len(3, caller_limit.get(), 1), 1);

    let committed = authority
        .plan_remote_expiry(RemoteDeadline(10), caller_limit)
        .expect("the complete due cut is valid")
        .expect("at least one Remote owner is due")
        .apply();
    assert_eq!(committed.retired_len(), 1);
    assert!(authority.entry(&first).is_none());
    assert!(authority.entry(&second).is_some());
    assert!(authority.entry(&third).is_some());
    let receipt = authority
        .effect_publication_receipt_for_foundation()
        .expect("the selected prefix publishes one exact effect batch");
    assert_eq!(
        receipt.effects(),
        &[CommittedEffect::RemoteExpired { tx_hash: first }]
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_operational_metrics_are_the_exact_read_only_owned_counter_projection() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let expired = admit_remote_until(&mut authority, 2_211, 211, 10);
    let retained = admit_remote_until(&mut authority, 2_212, 212, 20);
    let committed = authority
        .plan_remote_expiry(
            RemoteDeadline(10),
            NonZeroUsize::new(1).expect("one is non-zero"),
        )
        .expect("the indexed deadline cut is valid")
        .expect("one owner is due")
        .apply();
    assert_eq!(committed.retired_len(), 1);
    assert!(authority.entry(&expired).is_none());
    assert!(authority.entry(&retained).is_some());
    let receipt = authority
        .effect_publication_receipt_for_foundation()
        .expect("the committed expiry owns one resident effect batch");

    let resources = authority.resources().snapshot();
    let expected = exact_operational_projection(
        [
            resources.preaccepted.entries,
            resources.preaccepted.total_bytes().unwrap_or(usize::MAX),
            resources.remote.entries,
            resources.remote.total_bytes().unwrap_or(usize::MAX),
            resources.replacement_history.entries,
            resources
                .replacement_history
                .total_bytes()
                .unwrap_or(usize::MAX),
            resources.preaccepted.active_work,
        ],
        cumulative_effect_region_projection([1, receipt.charge_bytes()], [0, 0], [0, 0])
            .expect("one finite Remote batch has a cumulative region projection"),
    );
    let before = authority.normalized_snapshot();
    let metrics = authority.operational_metrics();
    let actual = [
        metrics.kernel.total_entries,
        metrics.kernel.total_bytes,
        metrics.kernel.remote_entries,
        metrics.kernel.remote_bytes,
        metrics.kernel.conflict_entries,
        metrics.kernel.conflict_bytes,
        metrics.kernel.active_work,
        metrics.effects.remote_batches,
        metrics.effects.remote_bytes,
        metrics.effects.ordinary_batches,
        metrics.effects.ordinary_bytes,
        metrics.effects.total_batches,
        metrics.effects.total_bytes,
    ];
    assert_eq!(actual, expected);
    assert_eq!(authority.normalized_snapshot(), before);
}
