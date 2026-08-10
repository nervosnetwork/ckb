use super::resource::{
    ContinuousAcceptedResources, ContinuousChargeError, ContinuousChargeRecord,
    ContinuousComputeLimits, ContinuousResourceChange, ContinuousResourceConfigError,
    ContinuousResourceLedger, ContinuousResourceLimits, ContinuousResourceVector,
    ModelComputeGrant,
};
use std::collections::BTreeMap;

fn limits() -> ContinuousResourceLimits {
    ContinuousResourceLimits::validate(
        ContinuousResourceVector {
            entries: 8,
            bytes: 80,
            edges: 40,
            active_work: 4,
            compute_bytes: 40,
            compute_edges: 20,
        },
        ContinuousResourceVector {
            entries: 6,
            bytes: 60,
            edges: 30,
            active_work: 3,
            compute_bytes: 30,
            compute_edges: 15,
        },
        ContinuousResourceVector {
            entries: 3,
            bytes: 30,
            edges: 15,
            active_work: 2,
            compute_bytes: 20,
            compute_edges: 10,
        },
        ContinuousResourceVector::retained(2, 20, 10),
        ContinuousAcceptedResources {
            entries: 8,
            serialized_bytes: 80,
            resident_bytes: 80,
            cycles: 80,
        },
        ContinuousComputeLimits {
            resolved_total_retained_bytes: 8,
            accepted_total_retained_bytes: 10,
            expanded_edges: 5,
        },
    )
    .expect("the finite limits form one monotonic hierarchy")
}

fn preaccepted(
    resources: ContinuousResourceVector,
    residency_peer: Option<u8>,
    compute_peer: Option<u8>,
) -> ContinuousChargeRecord {
    ContinuousChargeRecord::PreAccepted {
        resources,
        residency_peer,
        compute_peer,
    }
}

#[test]
fn model_charge_validation_exhausts_partial_compute_reservations() {
    for active_work in 0..=2 {
        for compute_bytes in 0..=2 {
            for compute_edges in 0..=2 {
                for residency_peer in [None, Some(1)] {
                    for compute_peer in [None, Some(1), Some(2)] {
                        let resources = ContinuousResourceVector {
                            entries: 1,
                            bytes: 2,
                            edges: 1,
                            active_work,
                            compute_bytes,
                            compute_edges,
                        };
                        let valid = active_work <= 1
                            && !(active_work == 0 && (compute_bytes != 0 || compute_edges != 0))
                            && !(active_work == 1 && compute_bytes == 0)
                            && compute_peer.is_none_or(|peer| Some(peer) == residency_peer)
                            && !(active_work == 0 && compute_peer.is_some());
                        assert_eq!(
                            preaccepted(resources, residency_peer, compute_peer)
                                .validate()
                                .is_ok(),
                            valid
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn model_resource_configuration_rejects_each_independent_invalid_premise() {
    let valid = limits();
    assert_eq!(valid.compute.expanded_edges, 5);

    let zero_compute = ContinuousResourceLimits::validate(
        valid.preaccepted,
        valid.remote,
        valid.per_peer,
        valid.replacement_history,
        valid.accepted,
        ContinuousComputeLimits {
            resolved_total_retained_bytes: 0,
            ..valid.compute
        },
    );
    assert_eq!(
        zero_compute,
        Err(ContinuousResourceConfigError::MissingComputeCapacity)
    );

    let nonmonotonic = ContinuousResourceLimits::validate(
        valid.preaccepted,
        valid.remote,
        valid.per_peer,
        valid.replacement_history,
        valid.accepted,
        ContinuousComputeLimits {
            resolved_total_retained_bytes: 11,
            accepted_total_retained_bytes: 10,
            expanded_edges: 5,
        },
    );
    assert_eq!(
        nonmonotonic,
        Err(ContinuousResourceConfigError::NonMonotonicComputeEnvelope)
    );

    let mut peer_too_large = valid.per_peer;
    peer_too_large.entries = valid.remote.entries + 1;
    assert_eq!(
        ContinuousResourceLimits::validate(
            valid.preaccepted,
            valid.remote,
            peer_too_large,
            valid.replacement_history,
            valid.accepted,
            valid.compute,
        ),
        Err(ContinuousResourceConfigError::LimitHierarchy)
    );

    let mut preaccepted_missing_envelope = valid.preaccepted;
    preaccepted_missing_envelope.compute_edges = valid.compute.expanded_edges - 1;
    let mut remote_missing_envelope = valid.remote;
    remote_missing_envelope.compute_edges = valid.compute.expanded_edges - 1;
    let mut peer_missing_envelope = valid.per_peer;
    peer_missing_envelope.compute_edges = valid.compute.expanded_edges - 1;
    assert_eq!(
        ContinuousResourceLimits::validate(
            preaccepted_missing_envelope,
            remote_missing_envelope,
            peer_missing_envelope,
            valid.replacement_history,
            valid.accepted,
            valid.compute,
        ),
        Err(ContinuousResourceConfigError::MissingComputeCapacity)
    );
}

#[test]
fn model_compute_reservation_is_all_or_nothing_and_preserves_edge_units() {
    let base = ContinuousResourceVector::retained(1, 4, 2);
    assert!(limits().compute.admits(base));
    let grant = ModelComputeGrant {
        total_retained_bytes: 8,
        edges: 5,
    };
    let reserved = base
        .reserve_compute(grant)
        .expect("an unreserved owner can consume one sealed compute grant");
    assert_eq!(reserved.active_work, 1);
    assert_eq!(reserved.total_bytes(), Some(12));
    assert_eq!(reserved.total_edges(), Some(7));
    assert!(!limits().compute.admits(reserved));
    assert!(reserved.reserve_compute(grant).is_none());

    for partial in [
        ContinuousResourceVector {
            active_work: 1,
            ..base
        },
        ContinuousResourceVector {
            compute_bytes: 1,
            ..base
        },
        ContinuousResourceVector {
            compute_edges: 1,
            ..base
        },
    ] {
        assert!(partial.reserve_compute(grant).is_none());
    }
}

#[test]
fn model_compute_release_consumes_the_exact_reservation_and_peer_attribution() {
    let base = ContinuousResourceVector::retained(1, 4, 2);
    let reserved = base
        .reserve_compute(ModelComputeGrant {
            total_retained_bytes: 8,
            edges: 5,
        })
        .expect("the compute grant is sealed");
    let old = preaccepted(reserved, Some(1), Some(1));
    let new = preaccepted(base, Some(1), None);
    let ledger = ContinuousResourceLedger::new(limits(), BTreeMap::from([(7, old)]))
        .expect("the exact remote charge fits the ledger");
    let released = ledger
        .plan_compute_release(7, old, new)
        .expect("the exact release only subtracts the compute reservation");
    assert_eq!(released.charges().get(&7), Some(&new));
    let usage = released.usage().expect("the released ledger remains valid");
    assert_eq!(usage.preaccepted, base);
    assert_eq!(usage.remote, base);
    assert_eq!(usage.per_peer.get(&1), Some(&base));

    let wrong_peer = preaccepted(base, Some(2), None);
    assert_eq!(
        ledger.plan_compute_release(7, old, wrong_peer),
        Err(ContinuousChargeError::InvalidComputeRelease)
    );
    let still_reserved = preaccepted(reserved, Some(1), Some(1));
    assert_eq!(
        ledger.plan_compute_release(7, old, still_reserved),
        Err(ContinuousChargeError::InvalidComputeRelease)
    );
}

#[test]
fn model_resource_batch_is_one_order_independent_set_transition() {
    let first = preaccepted(ContinuousResourceVector::retained(1, 30, 2), None, None);
    let second = preaccepted(ContinuousResourceVector::retained(1, 20, 2), None, None);
    let replacement = preaccepted(ContinuousResourceVector::retained(1, 40, 3), None, None);
    let ledger = ContinuousResourceLedger::new(
        limits(),
        BTreeMap::from([
            (1, first),
            (2, second),
            (
                3,
                ContinuousChargeRecord::ReplacementHistory(ContinuousResourceVector::retained(
                    1, 5, 1,
                )),
            ),
            (
                4,
                ContinuousChargeRecord::Accepted(ContinuousAcceptedResources {
                    entries: 1,
                    serialized_bytes: 5,
                    resident_bytes: 6,
                    cycles: 7,
                }),
            ),
        ]),
    )
    .expect("the initial set fits");
    let remove = ContinuousResourceChange {
        key: 1,
        expected: Some(first),
        after: None,
    };
    let replace = ContinuousResourceChange {
        key: 2,
        expected: Some(second),
        after: Some(replacement),
    };
    let left = ledger
        .plan_changes(&[remove, replace])
        .expect("the net set transition fits");
    let right = ledger
        .plan_changes(&[replace, remove])
        .expect("caller order cannot change a set transition");
    assert_eq!(left, right);
    assert_eq!(
        ledger.plan_changes(&[remove, remove]),
        Err(ContinuousChargeError::DuplicateChange)
    );
    assert_eq!(
        ledger.plan_changes(&[ContinuousResourceChange {
            key: 1,
            expected: Some(second),
            after: None,
        }]),
        Err(ContinuousChargeError::ExistingChargeMismatch)
    );
}

#[test]
fn model_membership_without_history_cannot_increase_transient_resource_usage() {
    let accepted = ContinuousChargeRecord::Accepted(ContinuousAcceptedResources {
        entries: 1,
        serialized_bytes: 5,
        resident_bytes: 6,
        cycles: 7,
    });
    let transient = ContinuousResourceVector::retained(1, 8, 3);
    let changed_before = [
        None,
        Some(preaccepted(transient, None, None)),
        Some(ContinuousChargeRecord::ReplacementHistory(transient)),
    ];

    for before in changed_before {
        for victim_count in 0..=2u8 {
            let mut charges = BTreeMap::new();
            if let Some(before) = before {
                charges.insert(0, before);
            }
            for victim in 0..victim_count {
                charges.insert(victim + 1, accepted);
            }
            let ledger = ContinuousResourceLedger::new(limits(), charges)
                .expect("the finite legal membership cut fits");
            let before_usage = ledger.usage().expect("the initial cut is valid");
            let mut changes = vec![ContinuousResourceChange {
                key: 0,
                expected: before,
                after: Some(accepted),
            }];
            changes.extend((0..victim_count).map(|victim| ContinuousResourceChange {
                key: victim + 1,
                expected: Some(accepted),
                after: None,
            }));
            let after = ledger
                .plan_changes(&changes)
                .expect("a no-history membership transition fits the transient partitions");
            let after_usage = after.usage().expect("the resulting cut is valid");

            assert!(after_usage.preaccepted.fits(before_usage.preaccepted));
            assert!(
                after_usage
                    .replacement_history
                    .fits(before_usage.replacement_history)
            );
        }
    }
}
