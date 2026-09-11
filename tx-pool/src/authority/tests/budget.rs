use super::super::{
    budget::{OwnerDelta, owner_amount},
    model::{Error, FullReason, Source, Status},
    store::Store,
};
use super::common::*;

#[test]
fn positive_reservation_rolls_back_when_dropped_and_exact_owner_charge_is_released() {
    let store = store();
    let candidate = entry(&store, tx(12), Source::Local);
    let accepted = super::common::verified(&store, &candidate, 1, 1, Status::Pending);
    let (plan, reject) =
        super::super::membership::admission(&store, &candidate, None, &accepted, &config(), true)
            .unwrap();
    assert!(reject.is_none());
    let after = plan
        .edits
        .values()
        .find_map(|edit| edit.after.as_ref())
        .unwrap();
    let charge = owner_amount(after).unwrap();
    let reservation = OwnerDelta::new(std::iter::empty(), std::iter::once(after.as_ref()))
        .unwrap()
        .reserve(&store.budget)
        .unwrap();
    assert_eq!(store.budget.accepted_usage(), charge);
    drop(reservation);
    assert_eq!(store.budget.accepted_usage().items, 0);
    store.apply(plan).unwrap();
    assert_eq!(store.budget.accepted_usage(), charge);
    store
        .apply(delete(&store, store.point(&candidate.hash()).1.unwrap()))
        .unwrap();
    assert_eq!(store.budget.accepted_usage().items, 0);
}

#[test]
fn remote_active_limits_leave_trusted_headroom_and_release_on_cancellation() {
    let store = store();
    let first = store.budget.active(remote(1, 1)).unwrap();
    assert!(matches!(
        store.budget.active(remote(1, 1)),
        Err(Error::Full(FullReason::Other("peer active work")))
    ));
    let second = store.budget.active(remote(2, 1)).unwrap();
    let resolver = store.budget.active(remote(3, 1)).unwrap();
    assert!(matches!(
        store.budget.active(remote(4, 1)),
        Err(Error::Full(_))
    ));
    let trusted = store.budget.active(Source::Local).unwrap();
    assert!(matches!(
        store.budget.active(Source::Local),
        Err(Error::Full(FullReason::Active))
    ));
    drop(first);
    let replacement = store.budget.active(remote(1, 1)).unwrap();
    drop((replacement, second, resolver, trusted));
    assert!(store.budget.active(Source::Local).is_ok());
    assert!(!store.budget.faulted());
}

#[test]
fn resource_coordinates_match_wide_arithmetic_at_every_machine_boundary() {
    use super::super::budget::Amount;
    let values = [
        0u64,
        1,
        2,
        u32::MAX as u64,
        u64::MAX / 2,
        u64::MAX - 1,
        u64::MAX,
    ];
    for a in values {
        for b in values {
            for coordinate in 0..5 {
                let make = |value: u64| {
                    let mut amount = Amount::default();
                    match coordinate {
                        0 => amount.items = value as usize,
                        1 => amount.bytes = value as usize,
                        2 => amount.edges = value as usize,
                        3 => amount.serialized = value as usize,
                        _ => amount.cycles = value,
                    }
                    amount
                };
                let maximum = if coordinate == 4 {
                    u64::MAX as u128
                } else {
                    usize::MAX as u128
                };
                let (a, b) = (a.min(maximum as u64), b.min(maximum as u64));
                let sum = a as u128 + b as u128;
                assert_eq!(
                    make(a).checked_add(make(b)),
                    (sum <= maximum).then(|| make(sum as u64))
                );
                assert_eq!(make(a).checked_sub(make(b)), (a >= b).then(|| make(a - b)));
                assert_eq!(make(a).fits(make(b)), a <= b);
            }
        }
    }
}

#[test]
fn active_capacity_return_wakes_registered_waiters() {
    use std::{
        future::Future,
        task::{Context, Waker},
    };
    let store = store();
    let mut changed = std::pin::pin!(store.budget.changed.notified());
    changed.as_mut().enable();
    let reservation = store.budget.active(Source::Local).unwrap();
    assert!(
        changed
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    drop(reservation);
    assert!(
        changed
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_ready()
    );
}

#[test]
fn active_pipeline_reserves_execution_population_within_the_derived_budget() {
    for workers in [1, 2, 8, 16] {
        let mut configuration = config();
        configuration.max_tx_verify_workers = workers;
        let store = Store::new(crate::test_support::genesis_snapshot(), &configuration).unwrap();
        let limits = &store.budget.limits;
        let total_bytes =
            crate::constants::ResidencyLimits::from_pool_size(configuration.max_tx_pool_size)
                .unwrap()
                .pipeline;
        // Keep the existing maximum job size; the added active envelope comes
        // out of queued residency, including its edge allowance.
        assert_eq!(limits.per_job.bytes, total_bytes / 4 / (workers + 1));
        assert_eq!(
            limits.pipeline.bytes + limits.per_job.bytes * (workers + 2),
            total_bytes
        );
        assert_eq!(
            limits.pipeline.edges + limits.per_job.edges * (workers + 2),
            total_bytes / 160
        );
        let mut remote_jobs: Vec<_> = (0..=workers)
            .map(|index| store.budget.active(remote(index % 4, 1)).unwrap())
            .collect();
        assert!(matches!(
            store.budget.active(remote(5, 1)),
            Err(Error::Full(FullReason::Other("remote active work")))
        ));
        let trusted = store.budget.active(Source::Local).unwrap();
        assert!(matches!(
            store.budget.active(Source::Local),
            Err(Error::Full(FullReason::Active))
        ));
        drop(remote_jobs.pop());
        let replacement = store.budget.active(remote(5, 1)).unwrap();
        drop((remote_jobs, replacement, trusted));
        let peer_jobs: Vec<_> = (0..(workers + 1).div_ceil(4))
            .map(|_| store.budget.active(remote(1, 1)).unwrap())
            .collect();
        assert!(matches!(
            store.budget.active(remote(1, 1)),
            Err(Error::Full(FullReason::Other("peer active work")))
        ));
        drop(peer_jobs);
        assert!(!store.budget.faulted());
    }
}

#[test]
fn serialized_capacity_keeps_its_units_and_small_pools_keep_execution_room() {
    let snapshot = crate::test_support::genesis_snapshot();
    for serialized in [0, 2_000, 180_000_000, 360_000_000] {
        let configuration = ckb_app_config::TxPoolConfig {
            max_tx_pool_size: serialized,
            max_tx_verify_workers: 2,
            ..Default::default()
        };
        let limits = super::super::budget::Limits::new(&configuration, snapshot.consensus())
            .expect("old serialized pool sizes remain usable");
        let scale = if serialized > 180_000_000 { 2 } else { 1 };
        assert_eq!(limits.accepted.serialized, serialized);
        assert_eq!(limits.accepted.bytes, 1_000_000_000 * scale);
        assert_eq!(limits.per_job.bytes, 32_000_000 * scale);
        assert_eq!(
            limits.pipeline.bytes + limits.per_job.bytes * 4,
            384_000_000 * scale
        );
    }
}

#[test]
fn unrepresentable_derived_residency_is_rejected_before_pool_creation() {
    let snapshot = crate::test_support::genesis_snapshot();
    let configuration = ckb_app_config::TxPoolConfig {
        max_tx_pool_size: usize::MAX,
        ..Default::default()
    };
    assert!(Store::new(snapshot, &configuration).is_err());
}

#[test]
fn four_peers_can_use_remote_slots_without_consuming_trusted_headroom() {
    let mut configuration = config();
    configuration.max_tx_verify_workers = 8;
    let store = Store::new(crate::test_support::genesis_snapshot(), &configuration).unwrap();
    let mut reservations = Vec::new();
    for peer in [1, 1, 1, 2, 2, 3, 3, 4, 4] {
        reservations.push(store.budget.active(remote(peer, 1)).unwrap());
    }
    assert!(matches!(
        store.budget.active(remote(5, 1)),
        Err(Error::Full(FullReason::Other("remote active work")))
    ));
    let trusted = store.budget.active(Source::Local).unwrap();
    assert!(matches!(
        store.budget.active(Source::Local),
        Err(Error::Full(FullReason::Active))
    ));
    drop((reservations, trusted));
    let first_peer = store.budget.active(remote(1, 1)).unwrap();
    let second_peer = store.budget.active(remote(1, 1)).unwrap();
    let third_peer = store.budget.active(remote(1, 1)).unwrap();
    assert!(matches!(
        store.budget.active(remote(1, 1)),
        Err(Error::Full(FullReason::Other("peer active work")))
    ));
    drop((first_peer, second_peer, third_peer));
    assert!(!store.budget.faulted());
}
