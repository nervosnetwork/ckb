//! Direct checks for bounded planning and read-only operational projections.

use super::foundation::{admit_remote_until, limits};
use crate::authority::{
    effect::{CommittedEffect, EffectLimits},
    plan::TxPoolAuthority,
    state::{ApplySequence, RemoteDeadline},
};
use std::num::NonZeroUsize;

#[test]
fn uak_remote_expiry_removes_exactly_the_effect_bounded_due_prefix() {
    let effect_limits =
        EffectLimits::for_foundation().with_remote_effects_per_batch_for_foundation(1);
    let mut authority = TxPoolAuthority::for_foundation_with_effect_limits(limits(), effect_limits)
        .expect("the one-effect Remote batch limit is valid");
    let first = admit_remote_until(&mut authority, 2_201, 201, 9);
    let second = admit_remote_until(&mut authority, 2_202, 202, 10);
    let third = admit_remote_until(&mut authority, 2_203, 203, 10);

    let committed = authority
        .plan_remote_expiry(
            RemoteDeadline(10),
            NonZeroUsize::new(3).expect("three is non-zero"),
        )
        .expect("the complete due cut is valid")
        .expect("at least one Remote owner is due")
        .apply_for_foundation(&authority);
    assert_eq!(committed.retired_len(), 1);
    assert!(authority.entry(&first).is_none());
    assert!(authority.entry(&second).is_some());
    assert!(authority.entry(&third).is_some());
    let receipt = authority
        .effect_publication_receipt_for_foundation()
        .expect("the selected prefix publishes one batch");
    assert_eq!(
        receipt.effects(),
        &[CommittedEffect::RemoteExpired { tx_hash: first }]
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_remote_expiry_commits_one_ordered_batch() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hashes = [
        admit_remote_until(&mut authority, 2_221, 221, 8),
        admit_remote_until(&mut authority, 2_222, 222, 9),
        admit_remote_until(&mut authority, 2_223, 223, 10),
    ];
    let before = authority.clocks().next_sequence;
    let committed = authority
        .plan_remote_expiry(
            RemoteDeadline(10),
            NonZeroUsize::new(3).expect("three is non-zero"),
        )
        .expect("batch expiry plans")
        .expect("three owners are due")
        .apply_for_foundation(&authority);
    assert_eq!(committed.retired_len(), hashes.len());
    assert!(hashes.iter().all(|hash| authority.entry(hash).is_none()));
    assert_eq!(
        authority.clocks().next_sequence,
        ApplySequence(before.0 + 1)
    );
    let receipt = authority
        .effect_publication_receipt_for_foundation()
        .expect("the batch publishes one ordered effect record");
    assert_eq!(
        receipt.effects(),
        &hashes
            .into_iter()
            .map(|tx_hash| CommittedEffect::RemoteExpired { tx_hash })
            .collect::<Vec<_>>()
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_operational_metrics_are_a_read_only_projection() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let expired = admit_remote_until(&mut authority, 2_211, 211, 10);
    let retained = admit_remote_until(&mut authority, 2_212, 212, 20);
    drop(
        authority
            .plan_remote_expiry(
                RemoteDeadline(10),
                NonZeroUsize::new(1).expect("one is non-zero"),
            )
            .expect("the expiry cut is valid")
            .expect("one owner is due")
            .apply_for_foundation(&authority),
    );
    assert!(authority.entry(&expired).is_none());
    assert!(authority.entry(&retained).is_some());
    let effect_bytes = authority
        .effect_publication_receipt_for_foundation()
        .expect("the expiry owns one effect batch")
        .charge_bytes();
    let resources = authority.resources().snapshot();
    let before = authority.normalized_snapshot();
    let metrics = authority.operational_metrics();

    assert_eq!(metrics.kernel.total_entries, resources.preaccepted.entries);
    assert_eq!(
        metrics.kernel.total_bytes,
        resources
            .preaccepted
            .total_bytes()
            .expect("fixture bytes fit")
    );
    assert_eq!(metrics.kernel.remote_entries, resources.remote.entries);
    assert_eq!(
        metrics.kernel.remote_bytes,
        resources.remote.total_bytes().expect("fixture bytes fit")
    );
    assert_eq!(
        metrics.kernel.conflict_entries,
        resources.replacement_history.entries
    );
    assert_eq!(
        metrics.kernel.conflict_bytes,
        resources
            .replacement_history
            .total_bytes()
            .expect("fixture bytes fit")
    );
    assert_eq!(
        metrics.kernel.active_work,
        resources.preaccepted.active_work
    );
    assert_eq!(metrics.effects.remote_batches, 1);
    assert_eq!(metrics.effects.remote_bytes, effect_bytes);
    assert_eq!(metrics.effects.ordinary_batches, 1);
    assert_eq!(metrics.effects.ordinary_bytes, effect_bytes);
    assert_eq!(metrics.effects.total_batches, 1);
    assert_eq!(metrics.effects.total_bytes, effect_bytes);
    assert_eq!(authority.normalized_snapshot(), before);
}
