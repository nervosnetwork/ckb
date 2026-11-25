use super::super::{
    budget::{AccountUsage, owner_amount},
    model::{Error, FullReason, Source, Status},
    store::Store,
};
use super::common::*;
use std::{
    future::Future,
    sync::Arc,
    task::{Context, Waker},
};

#[test]
fn inline_account_usage_tracks_admission_and_exact_owner_release() {
    let store = store();
    let candidate = entry(&store, tx(12), Source::Local);
    let mut active_changed = std::pin::pin!(store.budget.active_changed.notified());
    let mut context = Context::from_waker(Waker::noop());
    assert!(active_changed.as_mut().poll(&mut context).is_pending());
    let (_, reject) =
        commit_admission(&store, &candidate, 1, 1, Status::Pending, &config()).unwrap();
    assert!(reject.is_none());
    let accepted = store.point(&candidate.hash()).1.unwrap();
    let charge = owner_amount(&accepted);
    assert_eq!(store.accepted_usage(), charge);
    delete(&store, accepted).unwrap();
    assert_eq!(store.accepted_usage().items, 0);
    assert!(active_changed.as_mut().poll(&mut context).is_pending());
}

#[test]
fn invalid_owner_release_returns_fault_and_store_fault_wakes_active_waiters() {
    let store = store();
    let candidate = entry(&store, tx(13), Source::Local);
    let mut changed = std::pin::pin!(store.budget.active_changed.notified());
    let mut context = Context::from_waker(Waker::noop());
    assert!(changed.as_mut().poll(&mut context).is_pending());

    // Releasing an owner that was never charged fails with a structural fault.
    let mut usage = AccountUsage::default();
    assert!(matches!(
        usage.apply_edit(Some(&candidate), None),
        Err(Error::Fault("quota counter"))
    ));
    store.fault();
    assert!(changed.as_mut().poll(&mut context).is_ready());
    assert!(store.budget.faulted());
    assert!(matches!(
        store.budget.active(Source::Local),
        Err(Error::Fault("quota counter"))
    ));
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

    // When runtime concurrency grants a second trusted local compute permit,
    // both trusted headroom slots are available while remote stays at W + 1.
    let high_store =
        Store::with_runtime_workers(crate::test_support::genesis_snapshot(), &config(), Some(8))
            .unwrap();
    let r1 = high_store.budget.active(remote(1, 1)).unwrap();
    let r2 = high_store.budget.active(remote(2, 1)).unwrap();
    let r3 = high_store.budget.active(remote(3, 1)).unwrap();
    assert!(matches!(
        high_store.budget.active(remote(4, 1)),
        Err(Error::Full(_))
    ));
    let t1 = high_store.budget.active(Source::Local).unwrap();
    let t2 = high_store.budget.active(Source::Local).unwrap();
    assert!(matches!(
        high_store.budget.active(Source::Local),
        Err(Error::Full(FullReason::Active))
    ));
    drop((r1, r2, r3, t1, t2));
    assert!(!high_store.budget.faulted());
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
    let store = store();
    let mut changed = std::pin::pin!(store.budget.active_changed.notified());
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
    let snapshot = crate::test_support::genesis_snapshot();
    for workers in [0, 1, 2, 8, 16] {
        let mut configuration = config();
        configuration.max_tx_verify_workers = workers;
        let total_bytes =
            crate::constants::ResidencyLimits::from_pool_size(configuration.max_tx_pool_size)
                .unwrap()
                .pipeline;

        // 1. Baseline / insufficient runtime concurrency keeps `active_jobs = W + 2`
        //    so low-concurrency and `W = 0` deployments keep their full queued budget.
        for runtime_workers in [None, Some(2), Some(workers + 5)] {
            let expected_active =
                if workers > 0 && runtime_workers.is_some_and(|n| n >= workers + 6) {
                    workers + 3
                } else {
                    workers + 2
                };
            let store =
                Store::with_runtime_workers(Arc::clone(&snapshot), &configuration, runtime_workers)
                    .unwrap();
            let limits = &store.budget.limits;
            assert_eq!(limits.active_jobs, expected_active);
            assert_eq!(limits.per_job.bytes, total_bytes / 4 / (workers + 1));
            assert_eq!(
                limits.pipeline.bytes + limits.per_job.bytes * expected_active,
                total_bytes
            );
            assert_eq!(
                limits.pipeline.edges + limits.per_job.edges * expected_active,
                total_bytes / 160
            );
            let mut remote_jobs: Vec<_> = (0..=workers)
                .map(|index| store.budget.active(remote(index % 4, 1)).unwrap())
                .collect();
            assert!(matches!(
                store.budget.active(remote(5, 1)),
                Err(Error::Full(FullReason::Other("remote active work")))
            ));
            let trusted_extra = expected_active - (workers + 1);
            let trusted: Vec<_> = (0..trusted_extra)
                .map(|_| store.budget.active(Source::Local).unwrap())
                .collect();
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
                Err(Error::Full(FullReason::Other(
                    "peer active work" | "remote active work"
                )))
            ));
            drop(peer_jobs);
            assert!(!store.budget.faulted());
        }

        // 2. High runtime concurrency (`N >= W + 6`) grants the second extra
        //    local headroom slot (`W + 3`) only when `W > 0`, never when `W == 0`.
        let high_store =
            Store::with_runtime_workers(Arc::clone(&snapshot), &configuration, Some(workers + 6))
                .unwrap();
        let expected_high = if workers == 0 { 2 } else { workers + 3 };
        assert_eq!(high_store.budget.limits.active_jobs, expected_high);
        assert_eq!(
            high_store.budget.limits.per_job.bytes,
            total_bytes / 4 / (workers + 1)
        );
        assert_eq!(
            high_store.budget.limits.pipeline.bytes
                + high_store.budget.limits.per_job.bytes * expected_high,
            total_bytes
        );
        if workers == 0 {
            assert_eq!(high_store.budget.limits.pipeline.bytes, total_bytes / 2);
        }
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
        let high_limits = super::super::budget::Limits::with_runtime_workers(
            &configuration,
            snapshot.consensus(),
            Some(8),
        )
        .expect("high-concurrency runtime reserves second local headroom slot");
        assert_eq!(high_limits.active_jobs, 5);
        assert_eq!(high_limits.per_job.bytes, 32_000_000 * scale);
        assert_eq!(
            high_limits.pipeline.bytes + high_limits.per_job.bytes * 5,
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
